//! 主檔維護、單據過帳/作廢、移動加權平均成本重算。
use std::collections::{BTreeMap, BTreeSet};

use chrono::{Datelike, NaiveDate};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::db::{self, log, object, objects};
use crate::{Error, Result, bail};

pub const CATEGORIES: [&str; 6] = ["上衣", "褲/裙", "洋裝", "外套", "配件", "其他"];
pub const PAYMENTS: [&str; 4] = ["現金", "信用卡", "轉帳", "行動支付"];
pub const KINDS: [&str; 4] = ["盤點調整", "瑕疵報廢", "退回廠商", "其他"];

/// 每筆明細對庫存的增減：銷貨為出庫，進貨與調整依數量正負。
pub const EFFECT: &str = "(CASE d.type WHEN 'SO' THEN -l.qty ELSE l.qty END)";

const VARIANT: &str = "SELECT v.id, v.sku, v.qty_on_hand, v.avg_cost, v.safety_stock, \
    v.product_id, v.color_id, v.size_id, p.code product_code, p.name, p.list_price, p.image, \
    c.code color_code, c.name color, z.code size, z.sort size_sort, \
    (SELECT count(*) FROM line WHERE variant_id=v.id) used \
    FROM variant v JOIN product p ON p.id=v.product_id JOIN color c ON c.id=v.color_id JOIN size z ON z.id=v.size_id";

const PRODUCT: &str = "SELECT p.*, s.name supplier, \
    (SELECT coalesce(sum(qty_on_hand), 0) FROM variant WHERE product_id=p.id) stock, \
    (SELECT count(*) FROM variant WHERE product_id=p.id) skus \
    FROM product p LEFT JOIN supplier s ON s.id=p.supplier_id";

fn tx(c: &mut Connection) -> Result<rusqlite::Transaction<'_>> {
    Ok(c.transaction_with_behavior(TransactionBehavior::Immediate)?)
}

pub fn money(cents: i64) -> String {
    let (sign, c) = if cents < 0 { ("-", -cents) } else { ("", cents) };
    let int = (c / 100).to_string();
    let mut s = String::new();
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            s.push(',');
        }
        s.push(ch);
    }
    if c % 100 != 0 {
        s += &format!(".{:02}", c % 100);
    }
    format!("{sign}${s}")
}

/// 修改前後差異，寫進操作紀錄。標籤以「價」結尾的欄位視為金額。
fn diff(old: &Value, new: &Value, labels: &[(&str, &str)]) -> String {
    let show = |v: &Value, label: &str| match v {
        Value::String(s) if s.is_empty() => "（空）".into(),
        Value::String(s) => s.clone(),
        Value::Null => "（空）".into(),
        Value::Number(n) if label.ends_with('價') => money(n.as_i64().unwrap_or(0)),
        v => v.to_string(),
    };
    labels
        .iter()
        .filter(|(k, _)| old[k] != new[k])
        .map(|(k, l)| format!("{l}：{} → {}", show(&old[k], l), show(&new[k], l)))
        .collect::<Vec<_>>()
        .join("；")
}

// ---------- 廠商 ----------

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct Supplier {
    pub id: Option<i64>,
    pub code: String,
    pub name: String,
    pub contact: String,
    pub phone: String,
    pub address: String,
    pub note: String,
}

const SUPPLIER_LABELS: [(&str, &str); 6] =
    [("code", "代號"), ("name", "名稱"), ("contact", "聯絡人"), ("phone", "電話"), ("address", "地址"), ("note", "備註")];

pub fn suppliers(c: &Connection) -> Result<Vec<Value>> {
    objects(c, "SELECT s.*, (SELECT count(*) FROM product WHERE supplier_id=s.id) + \
        (SELECT count(*) FROM doc WHERE supplier_id=s.id) used FROM supplier s ORDER BY code", [])
}

pub fn save_supplier(c: &mut Connection, s: Supplier) -> Result<i64> {
    let new = json!({"code": s.code.trim(), "name": s.name.trim(), "contact": s.contact.trim(),
        "phone": s.phone.trim(), "address": s.address.trim(), "note": s.note.trim()});
    let (code, name) = (new["code"].as_str().unwrap_or(""), new["name"].as_str().unwrap_or(""));
    if code.is_empty() || name.is_empty() {
        return bail("請填寫廠商代號與名稱");
    }
    let tx = tx(c)?;
    let p = params![code, name, new["contact"].as_str(), new["phone"].as_str(), new["address"].as_str(), new["note"].as_str()];
    let id = match s.id {
        Some(id) => {
            let old = object(&tx, "SELECT code, name, contact, phone, address, note FROM supplier WHERE id=?1", [id])?;
            tx.execute("UPDATE supplier SET code=?1, name=?2, contact=?3, phone=?4, address=?5, note=?6 WHERE id=?7",
                rusqlite::params_from_iter(p.iter().copied().chain([&id as &dyn rusqlite::ToSql])))?;
            let d = diff(&old, &new, &SUPPLIER_LABELS);
            if !d.is_empty() {
                log(&tx, "修改廠商", code, &d)?;
            }
            id
        }
        None => {
            tx.execute("INSERT INTO supplier(code, name, contact, phone, address, note) VALUES(?1, ?2, ?3, ?4, ?5, ?6)", p)?;
            let id = tx.last_insert_rowid();
            log(&tx, "新增廠商", code, name)?;
            id
        }
    };
    tx.commit()?;
    Ok(id)
}

// ---------- 顏色、尺寸 ----------

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct Spec {
    pub id: Option<i64>,
    pub code: String,
    pub name: String,
    pub sort: i64,
}

pub fn colors(c: &Connection) -> Result<Vec<Value>> {
    objects(c, "SELECT c.*, (SELECT count(*) FROM variant WHERE color_id=c.id) used FROM color c ORDER BY code", [])
}

pub fn sizes(c: &Connection) -> Result<Vec<Value>> {
    objects(c, "SELECT z.*, (SELECT count(*) FROM variant WHERE size_id=z.id) used FROM size z ORDER BY sort, code", [])
}

fn spec_code(code: &str, what: &str) -> Result<String> {
    let code = code.trim().to_uppercase();
    if code.is_empty() || code.len() > 6 || !code.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        return bail(format!("{what}代碼請用 1~6 個英文字母或數字（會用在 SKU 上）"));
    }
    Ok(code)
}

pub fn save_color(c: &mut Connection, s: Spec) -> Result<i64> {
    let name = s.name.trim();
    if name.is_empty() {
        return bail("請填寫顏色名稱");
    }
    let tx = tx(c)?;
    let id = match s.id {
        Some(id) => {
            let (code, old): (String, String) =
                tx.query_row("SELECT code, name FROM color WHERE id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            if old != name {
                tx.execute("UPDATE color SET name=?2 WHERE id=?1", params![id, name])?;
                log(&tx, "修改顏色", &code, &format!("名稱：{old} → {name}"))?;
            }
            id
        }
        None => {
            let code = spec_code(&s.code, "顏色")?;
            tx.execute("INSERT INTO color(code, name) VALUES(?1, ?2)", params![code, name])?;
            let id = tx.last_insert_rowid();
            log(&tx, "新增顏色", &code, name)?;
            id
        }
    };
    tx.commit()?;
    Ok(id)
}

pub fn save_size(c: &mut Connection, s: Spec) -> Result<i64> {
    let tx = tx(c)?;
    let id = match s.id {
        Some(id) => {
            let (code, old): (String, i64) =
                tx.query_row("SELECT code, sort FROM size WHERE id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            if old != s.sort {
                tx.execute("UPDATE size SET sort=?2 WHERE id=?1", params![id, s.sort])?;
                log(&tx, "修改尺寸", &code, &format!("排序：{old} → {}", s.sort))?;
            }
            id
        }
        None => {
            let code = spec_code(&s.code, "尺寸")?;
            tx.execute("INSERT INTO size(code, sort) VALUES(?1, ?2)", params![code, s.sort])?;
            let id = tx.last_insert_rowid();
            log(&tx, "新增尺寸", &code, &format!("排序 {}", s.sort))?;
            id
        }
    };
    tx.commit()?;
    Ok(id)
}

/// 刪除未被使用的主檔；已被商品或單據引用時由外鍵擋下。
pub fn delete(c: &mut Connection, what: &str, id: i64) -> Result<()> {
    let (table, action, label) = match what {
        "supplier" => ("supplier", "刪除廠商", "code"),
        "color" => ("color", "刪除顏色", "code"),
        "size" => ("size", "刪除尺寸", "code"),
        "variant" => ("variant", "刪除 SKU", "sku"),
        "product" => ("product", "刪除商品", "code"),
        _ => return bail("未知的資料類型"),
    };
    let tx = tx(c)?;
    let target: String = tx.query_row(&format!("SELECT {label} FROM {table} WHERE id=?1"), [id], |r| r.get(0))?;
    if what == "product" {
        tx.execute("DELETE FROM variant WHERE product_id=?1", [id])?;
    }
    tx.execute(&format!("DELETE FROM {table} WHERE id=?1"), [id])?;
    log(&tx, action, &target, "")?;
    tx.commit()?;
    Ok(())
}

// ---------- 商品 ----------

/// 本店貨號：{YYYYMMDD}{3 碼當日流水號} 視為十進位整數，轉成大寫 16 進位。
/// 例：2026-10-17 第 1 款 → 20261017001 → 4B7A695A9
pub fn product_code(date: NaiveDate, serial: i64) -> Result<String> {
    if !(1..=999).contains(&serial) {
        return bail("今天建立的商品已達 999 款上限");
    }
    let n: u64 = format!("{}{serial:03}", date.format("%Y%m%d")).parse().unwrap_or(0);
    Ok(format!("{n:X}"))
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct ProductIn {
    pub id: Option<i64>,
    pub name: String,
    pub category: String,
    pub supplier_id: Option<i64>,
    pub supplier_code: String,
    pub list_price: i64,
    pub image: String,
    pub note: String,
    pub color_ids: Vec<i64>,
    pub size_ids: Vec<i64>,
}

const PRODUCT_LABELS: [(&str, &str); 7] = [("name", "品名"), ("category", "類別"), ("supplier", "廠商"),
    ("supplier_code", "廠商貨號"), ("list_price", "建議售價"), ("image", "主圖"), ("note", "備註")];

pub fn products(c: &Connection, q: &str) -> Result<Vec<Value>> {
    let q = q.trim();
    objects(c, &format!("{PRODUCT} WHERE ?1='' OR p.code LIKE ?2 OR p.name LIKE ?2 OR p.supplier_code LIKE ?2 \
        OR EXISTS(SELECT 1 FROM variant x WHERE x.product_id=p.id AND x.sku LIKE ?2) \
        ORDER BY p.code_date DESC, p.code_serial DESC LIMIT 300"), params![q, format!("%{q}%")])
}

fn variants_of(c: &Connection, product_id: i64) -> Result<Vec<Value>> {
    objects(c, &format!("{VARIANT} WHERE v.product_id=?1 ORDER BY c.code, z.sort, z.code"), [product_id])
}

pub fn product(c: &Connection, id: i64) -> Result<Value> {
    let mut p = object(c, &format!("{PRODUCT} WHERE p.id=?1"), [id])?;
    p["variants"] = variants_of(c, id)?.into();
    Ok(p)
}

pub fn save_product(c: &mut Connection, p: ProductIn) -> Result<i64> {
    let name = p.name.trim();
    if name.is_empty() {
        return bail("請填寫品名");
    }
    if !CATEGORIES.contains(&p.category.as_str()) {
        return bail("請選擇類別");
    }
    if p.list_price < 0 {
        return bail("建議售價不可為負數");
    }
    let tx = tx(c)?;
    let supplier: Option<String> = match p.supplier_id {
        Some(s) => Some(tx.query_row("SELECT name FROM supplier WHERE id=?1", [s], |r| r.get(0))
            .optional()?.ok_or_else(|| Error("找不到選擇的廠商".into()))?),
        None => None,
    };
    let new = json!({"name": name, "category": p.category, "supplier": supplier, "supplier_code": p.supplier_code.trim(),
        "list_price": p.list_price, "image": p.image, "note": p.note.trim()});
    let fields = params![name, p.category, p.supplier_id, p.supplier_code.trim(), p.list_price, p.image, p.note.trim()];
    let (id, code) = match p.id {
        Some(id) => {
            let old = object(&tx, "SELECT p.code, p.name, p.category, s.name supplier, p.supplier_code, p.list_price, \
                p.image, p.note FROM product p LEFT JOIN supplier s ON s.id=p.supplier_id WHERE p.id=?1", [id])?;
            tx.execute("UPDATE product SET name=?1, category=?2, supplier_id=?3, supplier_code=?4, list_price=?5, \
                image=?6, note=?7 WHERE id=?8",
                rusqlite::params_from_iter(fields.iter().copied().chain([&id as &dyn rusqlite::ToSql])))?;
            let code = old["code"].as_str().unwrap_or_default().to_string();
            let d = diff(&old, &new, &PRODUCT_LABELS);
            if !d.is_empty() {
                log(&tx, "修改商品", &code, &d)?;
            }
            (id, code)
        }
        None => {
            let today = db::today();
            let serial: i64 = tx.query_row("SELECT coalesce(max(code_serial), 0) + 1 FROM product WHERE code_date=?1",
                [today.to_string()], |r| r.get(0))?;
            let code = product_code(today, serial)?;
            tx.execute("INSERT INTO product(name, category, supplier_id, supplier_code, list_price, image, note, \
                code, code_date, code_serial) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                rusqlite::params_from_iter(fields.iter().copied().chain(
                    [&code as &dyn rusqlite::ToSql, &today.to_string(), &serial])))?;
            let id = tx.last_insert_rowid();
            log(&tx, "新增商品", &code, &format!("{name}（{}），建議售價 {}", p.category, money(p.list_price)))?;
            (id, code)
        }
    };
    insert_variants(&tx, id, &code, &p.color_ids, &p.size_ids)?;
    tx.commit()?;
    Ok(id)
}

fn insert_variants(tx: &Connection, id: i64, code: &str, colors: &[i64], sizes: &[i64]) -> Result<()> {
    let mut added = vec![];
    for color in colors {
        for size in sizes {
            let sku: Option<String> = tx.query_row(
                "INSERT OR IGNORE INTO variant(product_id, color_id, size_id, sku) \
                 SELECT ?1, c.id, z.id, ?2 || '-' || c.code || '-' || z.code FROM color c, size z \
                 WHERE c.id=?3 AND z.id=?4 RETURNING sku",
                params![id, code, color, size], |r| r.get(0)).optional()?;
            added.extend(sku);
        }
    }
    if !added.is_empty() {
        log(tx, "新增 SKU", code, &added.join("、"))?;
    }
    Ok(())
}

pub fn add_variants(c: &mut Connection, id: i64, colors: &[i64], sizes: &[i64]) -> Result<()> {
    let tx = tx(c)?;
    let code: String = tx.query_row("SELECT code FROM product WHERE id=?1", [id], |r| r.get(0))?;
    insert_variants(&tx, id, &code, colors, sizes)?;
    tx.commit()?;
    Ok(())
}

pub fn update_variant(c: &mut Connection, id: i64, safety_stock: i64) -> Result<()> {
    if safety_stock < 0 {
        return bail("安全庫存不可為負數");
    }
    let tx = tx(c)?;
    let (sku, old): (String, i64) =
        tx.query_row("SELECT sku, safety_stock FROM variant WHERE id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    if old != safety_stock {
        tx.execute("UPDATE variant SET safety_stock=?2 WHERE id=?1", params![id, safety_stock])?;
        log(&tx, "修改 SKU", &sku, &format!("安全庫存：{old} → {safety_stock}"))?;
    }
    tx.commit()?;
    Ok(())
}

/// 查詢：SKU 完全相符時回傳 variant；另依貨號或品名找商品（含各 SKU 庫存）。
pub fn find(c: &Connection, q: &str) -> Result<Value> {
    let q = q.trim();
    if q.is_empty() {
        return Ok(json!({"variant": null, "products": []}));
    }
    let variant = objects(c, &format!("{VARIANT} WHERE v.sku=?1 COLLATE NOCASE"), [q])?
        .into_iter().next();
    let head = q.split('-').next().unwrap_or(q);
    let mut products = objects(c, &format!("{PRODUCT} WHERE p.code=?1 COLLATE NOCASE"), [head])?;
    if products.is_empty() {
        products = objects(c, &format!("{PRODUCT} WHERE p.name LIKE ?1 OR p.supplier_code LIKE ?1 \
            ORDER BY p.code_date DESC, p.code_serial DESC LIMIT 20"), [format!("%{q}%")])?;
    }
    for p in &mut products {
        p["variants"] = variants_of(c, p["id"].as_i64().unwrap_or(0))?.into();
    }
    Ok(json!({"variant": variant, "products": products}))
}

// ---------- 單據 ----------

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct DocIn {
    #[serde(rename = "type")]
    pub doc_type: String,
    pub date: String,
    pub supplier_id: Option<i64>,
    pub payment: String,
    pub discount: i64,
    pub kind: String,
    pub reason: String,
    pub note: String,
    pub lines: Vec<LineIn>,
    pub photos: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct LineIn {
    pub variant_id: i64,
    pub qty: i64,
    pub unit_price: Option<i64>,
}

pub fn doc_name(t: &str) -> &'static str {
    match t {
        "PO" => "進貨單",
        "SO" => "銷貨單",
        _ => "庫存調整單",
    }
}

fn next_no(tx: &Connection, prefix: &str, date: NaiveDate) -> Result<String> {
    let head = format!("{prefix}{}", date.format("%Y%m%d"));
    let last: Option<String> =
        tx.query_row("SELECT max(no) FROM doc WHERE no LIKE ?1", [format!("{head}%")], |r| r.get(0))?;
    let n = last.map_or(1, |s| s[head.len()..].parse::<i64>().unwrap_or(0) + 1);
    if n > 999 {
        return bail("同一天的同類單據已達 999 張上限");
    }
    Ok(format!("{head}{n:03}"))
}

pub fn create_doc(c: &mut Connection, d: DocIn) -> Result<String> {
    let t = d.doc_type.as_str();
    let date = db::parse_date(&d.date)?;
    if date > db::today() {
        return bail("日期不可晚於今天");
    }
    if d.lines.is_empty() {
        return bail("請至少加入一筆商品");
    }
    if d.lines.iter().any(|l| l.unit_price.is_some_and(|p| p < 0)) {
        return bail("單價不可為負數");
    }
    match t {
        "PO" => {
            if d.supplier_id.is_none() {
                return bail("請選擇廠商");
            }
            if d.lines.iter().any(|l| l.qty <= 0) {
                return bail("進貨數量必須大於 0");
            }
            if d.lines.iter().any(|l| l.unit_price.is_none()) {
                return bail("請填寫每一筆的進貨單價");
            }
        }
        "SO" => {
            if !PAYMENTS.contains(&d.payment.as_str()) {
                return bail("請選擇付款方式");
            }
            if d.discount < 0 {
                return bail("折扣不可為負數");
            }
            if d.lines.iter().any(|l| l.qty == 0) {
                return bail("數量不可為 0");
            }
        }
        "AJ" => {
            if !KINDS.contains(&d.kind.as_str()) {
                return bail("請選擇調整類型");
            }
            if d.reason.trim().is_empty() {
                return bail("請填寫調整原因");
            }
            if d.lines.len() != 1 || d.lines[0].qty == 0 {
                return bail("請選擇一個 SKU，並填寫不為 0 的調整數量");
            }
        }
        _ => return bail("未知的單據類型"),
    }
    if d.photos.iter().any(|p| !p.starts_with("purchases/") || p.contains("..")) {
        return bail("照片路徑不正確，請重新選擇照片");
    }

    let tx = tx(c)?;
    let no = next_no(&tx, t, date)?;
    let (supplier_id, discount) = (if t == "PO" { d.supplier_id } else { None }, if t == "SO" { d.discount } else { 0 });
    tx.execute("INSERT INTO doc(type, no, date, supplier_id, payment, discount, kind, reason, note, created_at) \
        VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![t, no, date.to_string(), supplier_id, if t == "SO" { &d.payment } else { "" }, discount, if t == "AJ" { &d.kind } else { "" },
            d.reason.trim(), d.note.trim(), db::now()])?;
    let doc_id = tx.last_insert_rowid();

    let mut change: BTreeMap<i64, i64> = BTreeMap::new();
    let mut info: BTreeMap<i64, (String, String, i64)> = BTreeMap::new();
    let (mut detail, mut total, mut pieces) = (vec![], 0i64, 0i64);
    for l in &d.lines {
        let (sku, name, list_price, on_hand): (String, String, i64, i64) = tx.query_row(
            "SELECT v.sku, p.name, p.list_price, v.qty_on_hand FROM variant v JOIN product p ON p.id=v.product_id WHERE v.id=?1",
            [l.variant_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .optional()?.ok_or_else(|| Error("找不到明細中的 SKU，可能已被刪除，請重新加入".into()))?;
        let price = if t == "AJ" { 0 } else { l.unit_price.unwrap_or(list_price) };
        tx.execute("INSERT INTO line(doc_id, variant_id, qty, unit_price) VALUES(?1, ?2, ?3, ?4)",
            params![doc_id, l.variant_id, l.qty, price])?;
        *change.entry(l.variant_id).or_default() += if t == "SO" { -l.qty } else { l.qty };
        detail.push(if t == "AJ" { format!("{sku} {name} {:+}", l.qty) } else { format!("{sku} {name} ×{} @{}", l.qty, money(price)) });
        info.insert(l.variant_id, (sku, name, on_hand));
        total += l.qty * price;
        pieces += l.qty;
    }
    for (vid, delta) in &change {
        let (sku, name, on_hand) = &info[vid];
        if on_hand + delta < 0 {
            return bail(format!("庫存不足：{sku} {name} 目前只有 {on_hand} 件，這張單要減少 {} 件", -delta));
        }
    }
    for p in &d.photos {
        tx.execute("INSERT INTO photo(doc_id, image) VALUES(?1, ?2)", params![doc_id, p])?;
    }
    post(&tx, change.keys().copied(), "存檔")?;

    let head = match t {
        "PO" => format!("廠商 {}；{pieces} 件；合計 {}；照片 {} 張",
            tx.query_row("SELECT name FROM supplier WHERE id=?1", [supplier_id], |r| r.get::<_, String>(0))?,
            money(total), d.photos.len()),
        "SO" => format!("{}；{pieces} 件；折扣 {}；實收 {}", d.payment, money(discount), money(total - discount)),
        _ => format!("{}：{}", d.kind, d.reason.trim()),
    };
    log(&tx, &format!("新增{}", doc_name(t)), &no, &format!("{head}；明細：{}", detail.join("、")))?;
    tx.commit()?;
    Ok(no)
}

pub fn void_doc(c: &mut Connection, id: i64, reason: &str) -> Result<()> {
    let reason = reason.trim();
    if reason.is_empty() {
        return bail("請填寫作廢原因");
    }
    let tx = tx(c)?;
    let (t, no, status): (String, String, String) =
        tx.query_row("SELECT type, no, status FROM doc WHERE id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    if status != "posted" {
        return bail("這張單據已經作廢過了");
    }
    tx.execute("UPDATE doc SET status='void', void_reason=?2, voided_at=?3 WHERE id=?1", params![id, reason, db::now()])?;
    let ids: BTreeSet<i64> = db::rows(&tx, "SELECT DISTINCT variant_id FROM line WHERE doc_id=?1", [id])?
        .iter().filter_map(|r| r[0].as_i64()).collect();
    post(&tx, ids, "作廢")?;
    log(&tx, &format!("作廢{}", doc_name(&t)), &no, &format!("原因：{reason}"))?;
    tx.commit()?;
    Ok(())
}

/// 重算受影響的 SKU，任一 SKU 庫存變負數就整筆取消（交易回滾）。
fn post(tx: &Connection, ids: impl IntoIterator<Item = i64>, when: &str) -> Result<()> {
    let mut bad = vec![];
    for id in ids {
        recalc(tx, id)?;
        let (sku, q): (String, i64) =
            tx.query_row("SELECT sku, qty_on_hand FROM variant WHERE id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        if q < 0 {
            bad.push(format!("{sku}（會變成 {q} 件）"));
        }
    }
    if !bad.is_empty() {
        return bail(format!("{when}後庫存會變成負數，已取消：{}。請先處理相關的銷貨或調整單。", bad.join("、")));
    }
    Ok(())
}

/// 四捨五入的整數除法（遠離零）。
fn div_round(a: i64, b: i64) -> i64 {
    if a >= 0 { (2 * a + b) / (2 * b) } else { -((-2 * a + b) / (2 * b)) }
}

/// 依日期重播此 SKU 所有有效明細，算出每筆的成本、結存，以及目前庫存與平均成本（移動加權平均）。
/// 同一天先入庫後出庫，補登過去日期的單據也能得到正確成本。
pub fn recalc(tx: &Connection, variant_id: i64) -> Result<()> {
    let moves = db::rows(tx, &format!("SELECT l.id, d.type, {EFFECT}, l.unit_price FROM line l JOIN doc d ON d.id=l.doc_id \
        WHERE l.variant_id=?1 AND d.status='posted' ORDER BY d.date, {EFFECT} < 0, l.id"), [variant_id])?;
    let (mut qty, mut value, mut avg) = (0i64, 0i64, 0i64);
    let mut up = tx.prepare_cached("UPDATE line SET unit_cost=?2, balance_qty=?3, balance_value=?4 WHERE id=?1")?;
    for m in moves {
        let (id, eff, price) = (m[0].as_i64().unwrap_or(0), m[2].as_i64().unwrap_or(0), m[3].as_i64().unwrap_or(0));
        let cost = if m[1] == "PO" { price } else { avg }; // 銷貨、退貨、調整都以當下平均成本計
        qty += eff;
        value += eff * cost;
        if qty <= 0 {
            value = 0;
        } else {
            avg = div_round(value, qty);
        }
        up.execute(params![id, cost, qty, value])?;
    }
    tx.execute("UPDATE variant SET qty_on_hand=?2, avg_cost=?3 WHERE id=?1", params![variant_id, qty, avg])?;
    Ok(())
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct DocFilter {
    #[serde(rename = "type")]
    pub doc_type: String,
    pub start: String,
    pub end: String,
    pub q: String,
}

const DOC_LIST: &str = "SELECT d.id, d.type, d.no, d.date, d.status, d.payment, d.kind, d.reason, d.note, \
    d.void_reason, d.created_at, s.name supplier, \
    (SELECT count(*) FROM line WHERE doc_id=d.id) lines, \
    (SELECT coalesce(sum(qty), 0) FROM line WHERE doc_id=d.id) qty, \
    (SELECT coalesce(sum(qty * unit_price), 0) FROM line WHERE doc_id=d.id) - d.discount total, \
    (SELECT image FROM photo WHERE doc_id=d.id ORDER BY id LIMIT 1) photo \
    FROM doc d LEFT JOIN supplier s ON s.id=d.supplier_id";

pub fn docs(c: &Connection, f: DocFilter) -> Result<Vec<Value>> {
    let q = f.q.trim();
    let start = if f.start.is_empty() { "0000-00-00" } else { &f.start };
    let end = if f.end.is_empty() { "9999-99-99" } else { &f.end };
    objects(c, &format!("{DOC_LIST} WHERE (?1='' OR d.type=?1) AND d.date BETWEEN ?2 AND ?3 AND (?4='' OR d.no LIKE ?5 \
        OR EXISTS(SELECT 1 FROM line l JOIN variant v ON v.id=l.variant_id JOIN product p ON p.id=v.product_id \
        WHERE l.doc_id=d.id AND (v.sku LIKE ?5 OR p.code LIKE ?5 OR p.name LIKE ?5))) \
        ORDER BY d.date DESC, d.no DESC LIMIT 500"),
        params![f.doc_type, start, end, q, format!("%{q}%")])
}

pub fn doc(c: &Connection, id: i64) -> Result<Value> {
    let mut d = object(c, "SELECT d.*, s.name supplier FROM doc d LEFT JOIN supplier s ON s.id=d.supplier_id WHERE d.id=?1", [id])?;
    d["lines"] = objects(c, "SELECT l.*, l.qty * l.unit_price subtotal, v.sku, p.code product_code, p.name, p.image, \
        c.name color, z.code size FROM line l JOIN variant v ON v.id=l.variant_id JOIN product p ON p.id=v.product_id \
        JOIN color c ON c.id=v.color_id JOIN size z ON z.id=v.size_id WHERE l.doc_id=?1 ORDER BY l.id", [id])?.into();
    d["photos"] = objects(c, "SELECT image FROM photo WHERE doc_id=?1 ORDER BY id", [id])?.into();
    Ok(d)
}

// ---------- 首頁、核對、操作紀錄 ----------

/// 庫存欄位與單據明細加總不一致的 SKU。
pub fn mismatches(c: &Connection) -> Result<Vec<Value>> {
    objects(c, &format!("SELECT * FROM (SELECT v.id, v.sku, v.qty_on_hand, \
        (SELECT coalesce(sum({EFFECT}), 0) FROM line l JOIN doc d ON d.id=l.doc_id WHERE l.variant_id=v.id AND d.status='posted') expected \
        FROM variant v) WHERE qty_on_hand <> expected"), [])
}

pub fn recalc_all(c: &mut Connection) -> Result<String> {
    let bad = mismatches(c)?.len();
    let tx = tx(c)?;
    let ids: Vec<i64> = db::rows(&tx, "SELECT id FROM variant", [])?.iter().filter_map(|r| r[0].as_i64()).collect();
    for id in &ids {
        recalc(&tx, *id)?;
    }
    let msg = format!("共 {} 個 SKU 重新核算，修正 {bad} 個不一致", ids.len());
    log(&tx, "重新核算庫存", "", &msg)?;
    tx.commit()?;
    Ok(msg)
}

pub fn dashboard(c: &Connection) -> Result<Value> {
    let today = db::today();
    let month = today.with_day(1).unwrap_or(today);
    let sales = |from: NaiveDate| -> Result<(i64, i64, i64)> {
        Ok(c.query_row("SELECT count(*), coalesce(sum(t), 0), coalesce(sum(p), 0) FROM (SELECT \
            (SELECT coalesce(sum(qty * unit_price), 0) FROM line WHERE doc_id=d.id) - d.discount t, \
            (SELECT coalesce(sum(qty * (unit_price - unit_cost)), 0) FROM line WHERE doc_id=d.id) - d.discount p \
            FROM doc d WHERE d.type='SO' AND d.status='posted' AND d.date BETWEEN ?1 AND ?2)",
            [from.to_string(), today.to_string()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?)
    };
    let (today_count, today_sales, _) = sales(today)?;
    let (_, month_sales, month_profit) = sales(month)?;
    let (stock_qty, stock_value): (i64, i64) = c.query_row(
        "SELECT coalesce(sum(qty_on_hand), 0), coalesce(sum(qty_on_hand * avg_cost), 0) FROM variant WHERE qty_on_hand > 0",
        [], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(json!({
        "today_count": today_count, "today_sales": today_sales,
        "month_sales": month_sales, "month_profit": month_profit,
        "stock_qty": stock_qty, "stock_value": stock_value,
        "low": objects(c, &format!("{VARIANT} WHERE v.safety_stock > 0 AND v.qty_on_hand < v.safety_stock \
            ORDER BY v.qty_on_hand - v.safety_stock LIMIT 50"), [])?,
        "recent": objects(c, &format!("{DOC_LIST} ORDER BY d.id DESC LIMIT 8"), [])?,
        "mismatch": mismatches(c)?,
    }))
}

pub fn audit(c: &Connection, q: &str) -> Result<Vec<Value>> {
    let q = q.trim();
    objects(c, "SELECT * FROM audit WHERE ?1='' OR action LIKE ?2 OR target LIKE ?2 OR detail LIKE ?2 OR at LIKE ?2 \
        ORDER BY id DESC LIMIT 1000", params![q, format!("%{q}%")])
}
