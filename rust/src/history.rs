//! 匯入舊的 Excel 銷貨單（店內紙本版型或本系統匯出的銷貨單），只做歷史紀錄，不影響庫存。
//! 依表頭文字（品名、數量、單價、小計…）找欄位，不依固定儲存格位置。
use std::io::Cursor;

use calamine::{Data, Range, Reader, open_workbook_auto_from_rs};
use chrono::{Duration, NaiveDate};
use rusqlite::{Connection, TransactionBehavior, params};
use serde_json::{Value, json};

use crate::db::{self, log, object, objects};
use crate::store::money;
use crate::{Result, bail};

#[derive(Debug, Default)]
pub struct Sheet {
    pub no: String,
    pub date: Option<NaiveDate>,
    pub customer_name: String,
    pub customer_phone: String,
    pub customer_address: String,
    pub subtotal: Option<i64>,
    pub tax: Option<i64>,
    pub total: Option<i64>,
    pub lines: Vec<Line>,
}

#[derive(Debug, Default)]
pub struct Line {
    pub code: String,
    pub name: String,
    pub spec: String,
    pub qty: i64,
    pub unit_price: i64,
    pub subtotal: i64,
    pub note: String,
}

fn text(d: &Data) -> String {
    match d {
        Data::String(s) | Data::DateTimeIso(s) => s.trim().to_string(),
        Data::Int(i) => i.to_string(),
        Data::Float(f) if f.fract() == 0.0 => format!("{}", *f as i64),
        Data::Float(f) => f.to_string(),
        Data::DateTime(dt) => dt.as_datetime().map(|x| x.date().to_string()).unwrap_or_default(),
        Data::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

/// 標籤比對用：去掉全形與半形空白、冒號。
fn norm(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace() && *c != '\u{3000}' && *c != ':' && *c != '：').collect()
}

fn is_label(d: &Data) -> bool {
    let t = text(d);
    t.ends_with(':') || t.ends_with('：')
}

/// 金額轉成分；接受數字或「1,200」「$390」這類文字。
fn cents(d: &Data) -> Option<i64> {
    let f = match d {
        Data::Int(i) => *i as f64,
        Data::Float(f) => *f,
        Data::String(s) => s.replace([',', '$', '元', ' '], "").trim().parse().ok()?,
        _ => return None,
    };
    Some((f * 100.0).round() as i64)
}

/// 日期：Excel 日期、民國年（115/06/27、115.6.27、1150627）或西元年。
pub fn parse_date(d: &Data) -> Option<NaiveDate> {
    match d {
        Data::DateTime(dt) => return dt.as_datetime().map(|x| x.date()),
        Data::Float(f) if *f > 20_000.0 && *f < 80_000.0 => {
            return NaiveDate::from_ymd_opt(1899, 12, 30).map(|b| b + Duration::days(*f as i64));
        }
        _ => {}
    }
    let s = text(d);
    let parts: Vec<i32> = s.split(['/', '.', '-', '年', '月', '日']).filter(|p| !p.is_empty())
        .map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
    let (y, m, day) = match parts[..] {
        [y, m, d] => (y, m, d),
        [n] if s.len() == 7 => (n / 10000, n / 100 % 100, n % 100),
        [n] if s.len() == 8 => (n / 10000, n / 100 % 100, n % 100),
        _ => return None,
    };
    let y = if y < 1911 { y + 1911 } else { y };
    NaiveDate::from_ymd_opt(y, m as u32, day as u32)
}

/// 從一個工作表讀出銷貨單；沒有「品名＋數量」表頭的工作表回傳 None。
pub fn parse_sheet(range: &Range<Data>) -> Option<Sheet> {
    let rows: Vec<&[Data]> = range.rows().collect();
    // 標籤右邊第一個非空、且不是另一個標籤的儲存格
    let after = |label: &str| -> Option<&Data> {
        for row in &rows {
            if let Some(i) = row.iter().position(|c| norm(&text(c)) == label) {
                return row[i + 1..].iter().find(|c| !text(c).is_empty()).filter(|c| !is_label(c));
            }
        }
        None
    };
    let head = rows.iter().position(|r| {
        let n: Vec<String> = r.iter().map(|c| norm(&text(c))).collect();
        n.iter().any(|t| t == "品名") && n.iter().any(|t| t == "數量")
    })?;
    let col = |names: &[&str]| rows[head].iter().position(|c| names.contains(&norm(&text(c)).as_str()));
    let (c_name, c_qty) = (col(&["品名"])?, col(&["數量"])?);
    let (c_price, c_sub, c_note) = (col(&["單價"]), col(&["小計", "金額"]), col(&["備註"]));
    let (c_code, c_color, c_size) = (col(&["貨號", "本店貨號"]), col(&["顏色"]), col(&["尺寸"]));
    let get = |r: &[Data], c: Option<usize>| c.and_then(|c| r.get(c)).map(text).unwrap_or_default();
    let stop = ["經辦人", "銷貨總額", "合計件數", "總計", "合計"];

    let mut sheet = Sheet::default();
    for r in &rows[head + 1..] {
        if r.iter().any(|c| stop.contains(&norm(&text(c)).as_str())) {
            break;
        }
        let name = get(r, Some(c_name));
        let qty = r.get(c_qty).and_then(cents);
        if name.is_empty() && qty.is_none() {
            continue;
        }
        let qty = qty.map_or(0, |q| (q as f64 / 100.0).round() as i64);
        let price = c_price.and_then(|c| r.get(c)).and_then(cents).unwrap_or(0);
        let sub = c_sub.and_then(|c| r.get(c)).and_then(cents).unwrap_or(qty * price);
        let spec = [get(r, c_color), get(r, c_size)].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("/");
        sheet.lines.push(Line { code: get(r, c_code), name, spec, qty, unit_price: price, subtotal: sub, note: get(r, c_note) });
    }
    sheet.date = after("出貨日期").or_else(|| after("日期")).and_then(parse_date);
    sheet.no = after("貨單編號").or_else(|| after("單號")).map(text).unwrap_or_default();
    sheet.customer_name = after("客戶名稱").map(text).unwrap_or_default();
    sheet.customer_phone = after("電話").map(text).unwrap_or_default();
    sheet.customer_address = after("地址").map(text).unwrap_or_default();
    sheet.subtotal = after("銷貨總額").and_then(cents);
    sheet.tax = after("營業稅").and_then(cents);
    sheet.total = after("總計").and_then(cents);
    Some(sheet)
}

/// 匯入一個 Excel 檔（可含多個工作表）；已匯入過的單號會略過。
pub fn import(c: &mut Connection, file: &str, bytes: &[u8]) -> Result<Value> {
    let mut wb = open_workbook_auto_from_rs(Cursor::new(bytes.to_vec()))
        .map_err(|e| crate::Error(format!("無法讀取 {file}：{e}（請確認是 .xlsx 或 .xls 檔）")))?;
    let mut sheets = vec![];
    for name in wb.sheet_names() {
        if let Ok(range) = wb.worksheet_range(&name)
            && let Some(s) = parse_sheet(&range)
        {
            sheets.push((name, s));
        }
    }
    if sheets.is_empty() {
        return bail(format!("{file}：找不到銷貨單（需要有「品名」「數量」表頭）"));
    }
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (mut imported, mut skipped) = (vec![], vec![]);
    for (sheet_name, s) in sheets {
        let label = if s.no.is_empty() { format!("{file}／{sheet_name}") } else { s.no.clone() };
        let Some(date) = s.date else {
            skipped.push(format!("{label}：找不到出貨日期"));
            continue;
        };
        if s.lines.is_empty() && s.total.unwrap_or(0) == 0 {
            skipped.push(format!("{label}：沒有品項也沒有金額（空白單）"));
            continue;
        }
        let subtotal = s.subtotal.unwrap_or_else(|| s.lines.iter().map(|l| l.subtotal).sum());
        let tax = s.tax.unwrap_or(0);
        let total = s.total.unwrap_or(subtotal + tax);
        let key = if s.no.is_empty() { format!("檔案:{file}#{sheet_name}") } else { format!("單號:{}", s.no) };
        let n = tx.execute("INSERT OR IGNORE INTO history_doc(key, source, no, date, customer_name, customer_phone, \
            customer_address, subtotal, tax, total, imported_at) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![key, format!("{file}／{sheet_name}"), s.no, date.to_string(), s.customer_name, s.customer_phone,
                s.customer_address, subtotal, tax, total, db::now()])?;
        if n == 0 {
            skipped.push(format!("{label}：已經匯入過"));
            continue;
        }
        let id = tx.last_insert_rowid();
        for l in &s.lines {
            tx.execute("INSERT INTO history_line(history_id, code, name, spec, qty, unit_price, subtotal, note) \
                VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![id, l.code, l.name, l.spec, l.qty, l.unit_price, l.subtotal, l.note])?;
        }
        imported.push(format!("{label}（{date}，{} 項，總計 {}）", s.lines.len(), money(total)));
    }
    if !imported.is_empty() {
        log(&tx, "匯入歷史銷貨", file, &imported.join("、"))?;
    }
    tx.commit()?;
    Ok(json!({"imported": imported, "skipped": skipped}))
}

pub fn list(c: &Connection, start: &str, end: &str, q: &str) -> Result<Vec<Value>> {
    let q = q.trim();
    let start = if start.is_empty() { "0000-00-00" } else { start };
    let end = if end.is_empty() { "9999-99-99" } else { end };
    objects(c, "SELECT h.*, (SELECT count(*) FROM history_line WHERE history_id=h.id) lines, \
        (SELECT coalesce(sum(qty), 0) FROM history_line WHERE history_id=h.id) qty FROM history_doc h \
        WHERE h.date BETWEEN ?1 AND ?2 AND (?3='' OR h.no LIKE ?4 OR h.customer_name LIKE ?4 OR h.source LIKE ?4 \
        OR EXISTS(SELECT 1 FROM history_line l WHERE l.history_id=h.id AND (l.name LIKE ?4 OR l.code LIKE ?4))) \
        ORDER BY h.date DESC, h.id DESC LIMIT 1000", params![start, end, q, format!("%{q}%")])
}

pub fn get(c: &Connection, id: i64) -> Result<Value> {
    let mut h = object(c, "SELECT * FROM history_doc WHERE id=?1", [id])?;
    h["lines"] = objects(c, "SELECT * FROM history_line WHERE history_id=?1 ORDER BY id", [id])?.into();
    Ok(h)
}

/// 匯錯檔案時可刪除；歷史紀錄不影響庫存，刪除會寫進操作紀錄。
pub fn delete(c: &mut Connection, id: i64) -> Result<()> {
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let h = object(&tx, "SELECT no, date, source, total FROM history_doc WHERE id=?1", [id])?;
    tx.execute("DELETE FROM history_line WHERE history_id=?1", [id])?;
    tx.execute("DELETE FROM history_doc WHERE id=?1", [id])?;
    log(&tx, "刪除歷史銷貨", h["no"].as_str().unwrap_or_default(), &format!("{}，{}，總計 {}",
        h["date"].as_str().unwrap_or_default(), h["source"].as_str().unwrap_or_default(), money(h["total"].as_i64().unwrap_or(0))))?;
    tx.commit()?;
    Ok(())
}
