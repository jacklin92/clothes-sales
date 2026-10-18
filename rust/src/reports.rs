//! 報表：畫面與 Excel 共用同一份資料。
use std::path::{Path, PathBuf};

use chrono::{Datelike, Duration, NaiveDate};
use rusqlite::{Connection, params};
use rust_xlsxwriter::{ExcelDateTime, Format, FormatAlign, FormatBorder, Formula, Image, Workbook, Worksheet};
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
            &[("銷貨日期", Date), ("單號", Text), ("付款方式", Text), ("本店貨號", Text), ("品名", Text), ("顏色/尺寸", Text),
              ("數量", Int), ("單價", Money), ("小計", Money), ("成本", Money), ("毛利", Money)],
            rows(c, &format!("SELECT d.date, d.no, d.payment, p.code, p.name, c.name || '/' || z.code, l.qty, l.unit_price, \
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

const SHOP: &str = "衣の鋪";
const CLAUSE: &str = "***貸款未付清或票據未兌現償清之前，貨物之所有權仍歸屬本公司所有，買受人無異議，\
    同意本公司無需經法律程序隨時可取回本貨品或代物清償。***";
const MIN_ITEM_ROWS: usize = 7;

/// 單張銷貨單，沿用店內紙本出貨單版型；小計與合計用公式，在 Excel 改數量會自動重算。
pub fn export_sale(c: &Connection, dir: &Path, id: i64) -> Result<PathBuf> {
    let d = crate::store::doc(c, id)?;
    if d["type"] != "SO" {
        return bail("只有銷貨單可以匯出");
    }
    let s = |k: &str| d[k].as_str().unwrap_or_default().to_string();
    let lines = d["lines"].as_array().cloned().unwrap_or_default();
    let no = s("no");

    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    ws.set_name("銷貨單")?;
    let base = Format::new().set_font_size(12).set_align(FormatAlign::VerticalCenter);
    let label = base.clone().set_align(FormatAlign::Right);
    let field = base.clone().set_align(FormatAlign::Left).set_border_bottom(FormatBorder::Thin);
    let cell = base.clone().set_align(FormatAlign::Center).set_border(FormatBorder::Thin);
    let head = cell.clone().set_bold().set_background_color("#EFE6DA");
    let text = cell.clone().set_align(FormatAlign::Left);
    let money = cell.clone().set_align(FormatAlign::Right).set_num_format("#,##0");
    let total = money.clone().set_bold().set_font_size(14);
    let roc = field.clone().set_num_format("[$-404]e/mm/dd"); // 民國年，例 115/06/27

    for (col, w) in [11, 12, 20, 11, 7, 7, 11, 11, 13].into_iter().enumerate() {
        ws.set_column_width(col as u16, w)?;
    }
    ws.merge_range(0, 0, 0, 8, SHOP, &base.clone().set_font_size(22).set_bold().set_align(FormatAlign::Center))?;
    ws.set_row_height(0, 36)?;
    let title = if d["status"] == "void" { "銷 貨 單（已作廢）" } else { "銷 貨 單" };
    ws.merge_range(1, 0, 1, 8, title, &base.clone().set_font_size(14).set_align(FormatAlign::Center))?;

    ws.write_with_format(2, 0, "客戶名稱：", &label)?;
    ws.merge_range(2, 1, 2, 2, "", &field)?;
    ws.write_with_format(2, 3, "出貨日期：", &label)?;
    ws.merge_range(2, 4, 2, 5, "", &roc)?;
    ws.write_datetime_with_format(2, 4, ExcelDateTime::parse_from_str(&s("date"))?, &roc)?;
    ws.write_with_format(2, 6, "貨單編號：", &label)?;
    ws.merge_range(2, 7, 2, 8, &no, &field)?;
    ws.write_with_format(3, 0, "電　　話：", &label)?;
    ws.merge_range(3, 1, 3, 2, "", &field)?;
    ws.write_with_format(3, 3, "地　　址：", &label)?;
    ws.merge_range(3, 4, 3, 8, "", &field)?;
    ws.write_with_format(4, 0, "收款方式：", &label)?;
    ws.merge_range(4, 1, 4, 2, &s("payment"), &field)?;
    ws.write_with_format(4, 3, "備　　註：", &label)?;
    ws.merge_range(4, 4, 4, 8, &s("note"), &field)?;
    for row in 2..=4 {
        ws.set_row_height(row, 20)?;
    }

    for (col, h) in ["項次", "貨號", "品名", "顏色", "尺寸", "數量", "單價", "小計", "備註"].into_iter().enumerate() {
        ws.write_with_format(5, col as u16, h, &head)?;
    }
    let first = 6u32;
    let n = lines.len().max(MIN_ITEM_ROWS) as u32;
    for i in 0..n {
        let r = first + i;
        ws.set_row_height(r, 20)?;
        ws.write_with_format(r, 0, i + 1, &cell)?;
        match lines.get(i as usize) {
            Some(l) => {
                let ls = |k: &str| l[k].as_str().unwrap_or_default().to_string();
                let (qty, price) = (l["qty"].as_i64().unwrap_or(0), l["unit_price"].as_i64().unwrap_or(0));
                ws.write_with_format(r, 1, ls("product_code"), &cell)?;
                ws.write_with_format(r, 2, ls("name"), &text)?;
                ws.write_with_format(r, 3, ls("color"), &cell)?;
                ws.write_with_format(r, 4, ls("size"), &cell)?;
                ws.write_with_format(r, 5, qty as f64, &cell)?;
                ws.write_with_format(r, 6, price as f64 / 100.0, &money)?;
                let f = Formula::new(format!("=F{0}*G{0}", r + 1)).set_result((qty * price / 100).to_string());
                ws.write_formula_with_format(r, 7, f, &money)?;
                ws.write_with_format(r, 8, if qty < 0 { "退貨" } else { "" }, &text)?;
            }
            None => {
                for col in 1..=8 {
                    ws.write_blank(r, col, &cell)?;
                }
            }
        }
    }

    let t = first + n;
    let (sub, pcs): (i64, i64) = lines.iter().fold((0, 0), |(a, b), l| {
        (a + l["subtotal"].as_i64().unwrap_or(0), b + l["qty"].as_i64().unwrap_or(0))
    });
    let discount = d["discount"].as_i64().unwrap_or(0);
    let (top, bottom) = (first + 1, t);
    ws.merge_range(t, 0, t, 1, "經辦人", &cell)?;
    ws.merge_range(t, 2, t, 4, "", &cell)?;
    ws.merge_range(t, 5, t, 6, "銷貨總額", &cell)?;
    ws.write_formula_with_format(t, 7, Formula::new(format!("=SUM(H{top}:H{bottom})")).set_result((sub / 100).to_string()), &money)?;
    ws.write_blank(t, 8, &cell)?;
    ws.merge_range(t + 1, 0, t + 1, 1, "合計件數", &cell)?;
    ws.merge_range(t + 1, 2, t + 1, 4, "", &cell)?;
    ws.write_formula_with_format(t + 1, 2, Formula::new(format!("=SUM(F{top}:F{bottom})")).set_result(pcs.to_string()), &cell)?;
    ws.merge_range(t + 1, 5, t + 1, 6, "整單折扣", &cell)?;
    ws.write_with_format(t + 1, 7, discount as f64 / 100.0, &money)?;
    ws.write_blank(t + 1, 8, &cell)?;
    ws.merge_range(t + 2, 0, t + 2, 4, "", &cell)?;
    ws.merge_range(t + 2, 5, t + 2, 6, "總計", &head)?;
    ws.write_formula_with_format(t + 2, 7, Formula::new(format!("=H{}-H{}", t + 1, t + 2))
        .set_result(((sub - discount) / 100).to_string()), &total)?;
    ws.write_blank(t + 2, 8, &cell)?;
    for row in t..=t + 2 {
        ws.set_row_height(row, 22)?;
    }
    ws.merge_range(t + 3, 0, t + 3, 8, CLAUSE, &cell.clone().set_font_size(10).set_text_wrap())?;
    ws.set_row_height(t + 3, 34)?;

    ws.set_paper_size(9); // A4
    ws.set_print_fit_to_pages(1, 0);
    ws.set_print_area(0, 0, t + 3, 8)?;

    let path = dir.join("銷貨單").join(format!("{no}.xlsx"));
    save(&mut wb, &path)?;
    db::log(c, "匯出銷貨單", &no, &path.to_string_lossy())?;
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
