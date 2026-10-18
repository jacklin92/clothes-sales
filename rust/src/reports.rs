//! 報表：畫面與 Excel 共用同一份資料。
use std::path::{Path, PathBuf};

use chrono::{Datelike, Duration, NaiveDate};
use rusqlite::{Connection, params};
use rust_xlsxwriter::{ExcelDateTime, Format, Image, Workbook, Worksheet};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::{self, rows};
use crate::store::EFFECT;
use crate::{Error, Result, bail, media};

/// 欄位型別：img 圖片路徑、money 金額(分)、int 整數、date 日期、text 文字。
#[derive(Serialize)]
pub struct Report {
    pub label: &'static str,
    pub title: String,
    pub headers: Vec<(&'static str, &'static str)>,
    pub rows: Vec<Vec<Value>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct Params {
    pub start: String,
    pub end: String,
    pub days: i64,
}

const JOIN_V: &str = "JOIN variant v ON v.id=l.variant_id JOIN product p ON p.id=v.product_id \
    JOIN color c ON c.id=v.color_id JOIN size z ON z.id=v.size_id";

pub const THREE_FILE: &str = "庫存系統報表.xlsx";

pub fn build(c: &Connection, name: &str, p: &Params) -> Result<Report> {
    let today = db::today();
    let start = if p.start.is_empty() { today.with_day(1).unwrap_or(today) } else { db::parse_date(&p.start)? };
    let end = if p.end.is_empty() { today } else { db::parse_date(&p.end)? };
    let (s, e) = (start.to_string(), end.to_string());
    let days = if p.days > 0 { p.days } else { 60 };
    use Kind::*;
    Ok(match name {
        "inventory" => report("即時庫存表", "即時庫存表".into(),
            &[("圖片", Img), ("本店貨號", Text), ("SKU", Text), ("品名", Text), ("顏色", Text), ("尺寸", Text),
              ("目前庫存", Int), ("平均成本", Money), ("庫存金額", Money), ("最後進貨日", Date), ("安全庫存", Int), ("警示", Text)],
            rows(c, "SELECT p.image, p.code, v.sku, p.name, c.name, z.code, v.qty_on_hand, v.avg_cost, v.qty_on_hand * v.avg_cost, \
                (SELECT max(d.date) FROM line l JOIN doc d ON d.id=l.doc_id WHERE l.variant_id=v.id AND d.type='PO' AND d.status='posted'), \
                v.safety_stock, CASE WHEN v.qty_on_hand < v.safety_stock THEN '低於安全庫存' ELSE '' END \
                FROM variant v JOIN product p ON p.id=v.product_id JOIN color c ON c.id=v.color_id JOIN size z ON z.id=v.size_id \
                ORDER BY p.code, c.code, z.sort", [])?),
        "purchases" => report("進貨報表", format!("進貨報表 {s} ~ {e}"),
            &[("進貨日期", Date), ("單號", Text), ("廠商", Text), ("品名", Text), ("顏色/尺寸", Text), ("數量", Int),
              ("單價", Money), ("總成本", Money), ("廠商貨號", Text), ("本店貨號", Text), ("照片", Img)],
            rows(c, &format!("SELECT d.date, d.no, s.name, p.name, c.name || '/' || z.code, l.qty, l.unit_price, l.qty * l.unit_price, \
                p.supplier_code, p.code, (SELECT image FROM photo WHERE doc_id=d.id ORDER BY id LIMIT 1) \
                FROM line l JOIN doc d ON d.id=l.doc_id JOIN supplier s ON s.id=d.supplier_id {JOIN_V} \
                WHERE d.type='PO' AND d.status='posted' AND d.date BETWEEN ?1 AND ?2 ORDER BY d.date, d.no, l.id"), [&s, &e])?),
        "sales" => report("銷貨報表", format!("銷貨報表 {s} ~ {e}"),
            &[("銷貨日期", Date), ("單號", Text), ("通路", Text), ("本店貨號", Text), ("品名", Text), ("顏色/尺寸", Text),
              ("數量", Int), ("單價", Money), ("小計", Money), ("成本", Money), ("毛利", Money)],
            rows(c, &format!("SELECT d.date, d.no, d.channel, p.code, p.name, c.name || '/' || z.code, l.qty, l.unit_price, \
                l.qty * l.unit_price, l.qty * l.unit_cost, l.qty * (l.unit_price - l.unit_cost) \
                FROM line l JOIN doc d ON d.id=l.doc_id {JOIN_V} \
                WHERE d.type='SO' AND d.status='posted' AND d.date BETWEEN ?1 AND ?2 ORDER BY d.date, d.no, l.id"), [&s, &e])?),
        "quarterly" => quarterly(c, today)?,
        "slow" => {
            let cutoff = (today - Duration::days(days)).to_string();
            report("滯銷品", format!("滯銷品（{days} 天內無銷售且有庫存）"),
                &[("圖片", Img), ("本店貨號", Text), ("SKU", Text), ("品名", Text), ("顏色", Text), ("尺寸", Text),
                  ("目前庫存", Int), ("最後銷貨日", Date), ("首次進貨日", Date)],
                rows(c, "SELECT p.image, p.code, v.sku, p.name, c.name, z.code, v.qty_on_hand, v.ls, v.fi FROM (SELECT v.*, \
                    (SELECT max(d.date) FROM line l JOIN doc d ON d.id=l.doc_id WHERE l.variant_id=v.id AND d.type='SO' AND d.status='posted') ls, \
                    (SELECT min(d.date) FROM line l JOIN doc d ON d.id=l.doc_id WHERE l.variant_id=v.id AND d.type='PO' AND d.status='posted') fi \
                    FROM variant v WHERE v.qty_on_hand > 0) v \
                    JOIN product p ON p.id=v.product_id JOIN color c ON c.id=v.color_id JOIN size z ON z.id=v.size_id \
                    WHERE coalesce(v.ls, v.fi, ?1) <= ?2 ORDER BY coalesce(v.ls, v.fi)", [today.to_string(), cutoff])?)
        }
        "top" => report("熱銷排行", format!("熱銷排行 {s} ~ {e}"),
            &[("圖片", Img), ("本店貨號", Text), ("品名", Text), ("銷售數量", Int), ("銷售額", Money)],
            rows(c, &format!("SELECT p.image, p.code, p.name, sum(l.qty) q, sum(l.qty * l.unit_price) \
                FROM line l JOIN doc d ON d.id=l.doc_id {JOIN_V} \
                WHERE d.type='SO' AND d.status='posted' AND d.date BETWEEN ?1 AND ?2 \
                GROUP BY p.id ORDER BY q DESC LIMIT 50"), [&s, &e])?),
        _ => return bail("未知的報表"),
    })
}

#[derive(Clone, Copy)]
enum Kind { Img, Text, Int, Money, Date }

fn report(label: &'static str, title: String, headers: &[(&'static str, Kind)], rows: Vec<Vec<Value>>) -> Report {
    let kind = |k: Kind| match k { Kind::Img => "img", Kind::Text => "text", Kind::Int => "int", Kind::Money => "money", Kind::Date => "date" };
    Report { label, title, headers: headers.iter().map(|(h, k)| (*h, kind(*k))).collect(), rows }
}

/// 某日結束時的庫存總金額：每個 SKU 取當日以前最後一筆明細的結存金額。
fn inventory_value(c: &Connection, as_of: NaiveDate) -> Result<i64> {
    Ok(c.query_row(&format!("SELECT coalesce(sum(bv), 0) FROM (SELECT l.balance_value bv, row_number() OVER \
        (PARTITION BY l.variant_id ORDER BY d.date DESC, {EFFECT} < 0 DESC, l.id DESC) rn \
        FROM line l JOIN doc d ON d.id=l.doc_id WHERE d.status='posted' AND d.date <= ?1) WHERE rn=1"),
        [as_of.to_string()], |r| r.get(0))?)
}

fn quarter_start(d: NaiveDate) -> NaiveDate {
    NaiveDate::from_ymd_opt(d.year(), (d.month() - 1) / 3 * 3 + 1, 1).unwrap_or(d)
}

fn next_quarter(q: NaiveDate) -> NaiveDate {
    let (y, m) = if q.month() >= 10 { (q.year() + 1, 1) } else { (q.year(), q.month() + 3) };
    NaiveDate::from_ymd_opt(y, m, 1).unwrap_or(q)
}

fn quarterly(c: &Connection, today: NaiveDate) -> Result<Report> {
    use Kind::*;
    let mut r = report("季度收益", "季度收益".into(),
        &[("季度", Text), ("進貨成本", Money), ("銷貨總額", Money), ("銷貨成本", Money), ("毛利", Money), ("毛利率", Text),
          ("期初庫存", Money), ("期末庫存", Money), ("週轉率", Text)], vec![]);
    let first: Option<String> = c.query_row("SELECT min(date) FROM doc WHERE status='posted'", [], |r| r.get(0))?;
    let Some(first) = first else { return Ok(r) };
    let mut q = quarter_start(db::parse_date(&first)?);
    let sum = |sql: &str, a: &str, b: &str| -> Result<i64> { Ok(c.query_row(sql, params![a, b], |r| r.get(0))?) };
    while q <= today {
        let end = next_quarter(q) - Duration::days(1);
        let (a, b) = (q.to_string(), end.to_string());
        let line_sum = |t: &str, expr: &str| sum(&format!("SELECT coalesce(sum({expr}), 0) FROM line l JOIN doc d ON d.id=l.doc_id \
            WHERE d.type='{t}' AND d.status='posted' AND d.date BETWEEN ?1 AND ?2"), &a, &b);
        let buy = line_sum("PO", "l.qty * l.unit_price")?;
        let gross = line_sum("SO", "l.qty * l.unit_price")?;
        let cogs = line_sum("SO", "l.qty * l.unit_cost")?;
        let discount = sum("SELECT coalesce(sum(discount), 0) FROM doc WHERE type='SO' AND status='posted' AND date BETWEEN ?1 AND ?2", &a, &b)?;
        let revenue = gross - discount;
        let opening = inventory_value(c, q - Duration::days(1))?;
        let closing = inventory_value(c, end)?;
        let avg_inv = (opening + closing) as f64 / 2.0;
        r.rows.push(vec![
            format!("{} Q{}", q.year(), (q.month() - 1) / 3 + 1).into(), buy.into(), revenue.into(), cogs.into(),
            (revenue - cogs).into(),
            if revenue != 0 { format!("{:.1}%", (revenue - cogs) as f64 * 100.0 / revenue as f64) } else { "-".into() }.into(),
            opening.into(), closing.into(),
            if avg_inv > 0.0 { format!("{:.2}", cogs as f64 / avg_inv) } else { "-".into() }.into(),
        ]);
        q = next_quarter(q);
    }
    Ok(r)
}

/// 對應系統規劃.xlsx 的三個工作表（全部期間）。
pub fn three(c: &Connection) -> Result<Vec<(&'static str, Report)>> {
    let p = Params { start: "2000-01-01".into(), end: db::today().to_string(), days: 60 };
    Ok(vec![("進貨系統", build(c, "purchases", &p)?), ("銷貨系統", build(c, "sales", &p)?), ("收益計算", build(c, "quarterly", &p)?)])
}

fn write_sheet(ws: &mut Worksheet, rep: &Report, name: &str, media_dir: &Path) -> Result<()> {
    ws.set_name(name.chars().take(31).collect::<String>())?;
    let bold = Format::new().set_bold();
    let money = Format::new().set_num_format("#,##0.00");
    let date = Format::new().set_num_format("yyyy-mm-dd");
    for (col, (h, kind)) in rep.headers.iter().enumerate() {
        let col = col as u16;
        ws.write_string_with_format(0, col, *h, &bold)?;
        ws.set_column_width(col, if *kind == "img" { 12.0 } else { (h.chars().count() * 2 + 4).max(10) as f64 })?;
    }
    for (r, row) in rep.rows.iter().enumerate() {
        let r = r as u32 + 1;
        for (col, v) in row.iter().enumerate() {
            let kind = rep.headers[col].1;
            let col = col as u16;
            match (kind, v) {
                ("img", Value::String(p)) if !p.is_empty() => {
                    if let Ok(buf) = media::thumbnail(&media_dir.join(p), 80) {
                        ws.insert_image_fit_to_cell(r, col, &Image::new_from_buffer(&buf)?, true)?;
                        ws.set_row_height(r, 62)?;
                    }
                }
                ("money", Value::Number(n)) => { ws.write_number_with_format(r, col, n.as_f64().unwrap_or(0.0) / 100.0, &money)?; }
                ("int", Value::Number(n)) => { ws.write_number(r, col, n.as_f64().unwrap_or(0.0))?; }
                ("date", Value::String(s)) => { ws.write_datetime_with_format(r, col, ExcelDateTime::parse_from_str(s)?, &date)?; }
                (_, Value::String(s)) => { ws.write_string(r, col, s)?; }
                _ => {}
            }
        }
    }
    Ok(())
}

fn save(wb: &mut Workbook, path: &Path) -> Result<()> {
    std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
    wb.save(path).map_err(|e| Error(format!(
        "無法寫入 {}。若這個檔案已用 Excel 開啟，請先關閉再試一次。（{e}）",
        path.file_name().unwrap_or_default().to_string_lossy())))
}

pub fn export(c: &Connection, report_dir: &Path, media_dir: &Path, name: &str, p: &Params) -> Result<PathBuf> {
    let rep = build(c, name, p)?;
    let mut wb = Workbook::new();
    write_sheet(wb.add_worksheet(), &rep, rep.label, media_dir)?;
    let path = report_dir.join(format!("{}_{}.xlsx", rep.label, db::today().format("%Y%m%d")));
    save(&mut wb, &path)?;
    db::log(c, "匯出報表", rep.label, &path.to_string_lossy())?;
    Ok(path)
}

pub fn export_three(c: &Connection, report_dir: &Path, media_dir: &Path) -> Result<PathBuf> {
    let mut wb = Workbook::new();
    for (sheet, rep) in three(c)? {
        write_sheet(wb.add_worksheet(), &rep, sheet, media_dir)?;
    }
    let path = report_dir.join(THREE_FILE);
    save(&mut wb, &path)?;
    db::log(c, "更新三大報表", THREE_FILE, &path.to_string_lossy())?;
    Ok(path)
}
