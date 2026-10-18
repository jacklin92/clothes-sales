//! 資料庫：結構、防竄改規則、備份與共用查詢工具。
//! 金額一律以「分」(整數) 儲存，避免浮點誤差。
use std::path::Path;

use chrono::{Local, NaiveDate};
use rusqlite::{Connection, Params, params, types::ValueRef};
use serde_json::{Map, Value};

use crate::{Error, Result, bail};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS supplier(
  id INTEGER PRIMARY KEY,
  code TEXT NOT NULL UNIQUE CHECK(code<>''),
  name TEXT NOT NULL CHECK(name<>''),
  contact TEXT NOT NULL DEFAULT '', phone TEXT NOT NULL DEFAULT '',
  address TEXT NOT NULL DEFAULT '', note TEXT NOT NULL DEFAULT '');
CREATE TABLE IF NOT EXISTS color(
  id INTEGER PRIMARY KEY, code TEXT NOT NULL UNIQUE CHECK(code<>''), name TEXT NOT NULL CHECK(name<>''));
CREATE TABLE IF NOT EXISTS size(
  id INTEGER PRIMARY KEY, code TEXT NOT NULL UNIQUE CHECK(code<>''), sort INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS product(
  id INTEGER PRIMARY KEY,
  code TEXT NOT NULL UNIQUE, code_date TEXT NOT NULL, code_serial INTEGER NOT NULL,
  name TEXT NOT NULL CHECK(name<>''), category TEXT NOT NULL,
  supplier_id INTEGER REFERENCES supplier(id), supplier_code TEXT NOT NULL DEFAULT '',
  list_price INTEGER NOT NULL DEFAULT 0 CHECK(list_price>=0),
  image TEXT NOT NULL DEFAULT '', note TEXT NOT NULL DEFAULT '',
  UNIQUE(code_date, code_serial));
CREATE TABLE IF NOT EXISTS variant(
  id INTEGER PRIMARY KEY,
  product_id INTEGER NOT NULL REFERENCES product(id),
  color_id INTEGER NOT NULL REFERENCES color(id),
  size_id INTEGER NOT NULL REFERENCES size(id),
  sku TEXT NOT NULL UNIQUE, barcode TEXT NOT NULL DEFAULT '',
  safety_stock INTEGER NOT NULL DEFAULT 0 CHECK(safety_stock>=0),
  qty_on_hand INTEGER NOT NULL DEFAULT 0, avg_cost INTEGER NOT NULL DEFAULT 0,
  UNIQUE(product_id, color_id, size_id));
CREATE UNIQUE INDEX IF NOT EXISTS variant_barcode ON variant(barcode) WHERE barcode<>'';
CREATE TABLE IF NOT EXISTS doc(
  id INTEGER PRIMARY KEY,
  type TEXT NOT NULL CHECK(type IN ('PO','SO','AJ')),
  no TEXT NOT NULL UNIQUE, date TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'posted' CHECK(status IN ('posted','void')),
  supplier_id INTEGER REFERENCES supplier(id),
  channel TEXT NOT NULL DEFAULT '', payment TEXT NOT NULL DEFAULT '',
  discount INTEGER NOT NULL DEFAULT 0 CHECK(discount>=0),
  kind TEXT NOT NULL DEFAULT '', reason TEXT NOT NULL DEFAULT '', note TEXT NOT NULL DEFAULT '',
  void_reason TEXT NOT NULL DEFAULT '', created_at TEXT NOT NULL, voided_at TEXT NOT NULL DEFAULT '',
  CHECK(type<>'PO' OR supplier_id IS NOT NULL));
CREATE INDEX IF NOT EXISTS doc_date ON doc(date);
CREATE TABLE IF NOT EXISTS line(
  id INTEGER PRIMARY KEY,
  doc_id INTEGER NOT NULL REFERENCES doc(id),
  variant_id INTEGER NOT NULL REFERENCES variant(id),
  qty INTEGER NOT NULL CHECK(qty<>0),
  unit_price INTEGER NOT NULL DEFAULT 0 CHECK(unit_price>=0),
  unit_cost INTEGER NOT NULL DEFAULT 0, balance_qty INTEGER NOT NULL DEFAULT 0,
  balance_value INTEGER NOT NULL DEFAULT 0);
CREATE INDEX IF NOT EXISTS line_variant ON line(variant_id);
CREATE INDEX IF NOT EXISTS line_doc ON line(doc_id);
CREATE TABLE IF NOT EXISTS photo(
  id INTEGER PRIMARY KEY, doc_id INTEGER NOT NULL REFERENCES doc(id), image TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS audit(
  id INTEGER PRIMARY KEY, at TEXT NOT NULL, action TEXT NOT NULL,
  target TEXT NOT NULL DEFAULT '', detail TEXT NOT NULL DEFAULT '');

-- 單據、明細、照片、操作紀錄只能新增；單據只能改狀態為作廢。即使程式出錯也改不動。
CREATE TRIGGER IF NOT EXISTS doc_no_delete BEFORE DELETE ON doc
  BEGIN SELECT RAISE(ABORT, '單據不可刪除，請改用作廢'); END;
CREATE TRIGGER IF NOT EXISTS doc_locked BEFORE UPDATE OF
  type, no, date, supplier_id, channel, payment, discount, kind, reason, note, created_at ON doc
  BEGIN SELECT RAISE(ABORT, '單據建立後不可修改，打錯請作廢後重開'); END;
CREATE TRIGGER IF NOT EXISTS doc_no_unvoid BEFORE UPDATE OF status ON doc WHEN OLD.status='void'
  BEGIN SELECT RAISE(ABORT, '已作廢的單據不能恢復'); END;
CREATE TRIGGER IF NOT EXISTS line_no_delete BEFORE DELETE ON line
  BEGIN SELECT RAISE(ABORT, '單據明細不可刪除'); END;
CREATE TRIGGER IF NOT EXISTS line_locked BEFORE UPDATE OF doc_id, variant_id, qty, unit_price ON line
  BEGIN SELECT RAISE(ABORT, '單據明細不可修改'); END;
CREATE TRIGGER IF NOT EXISTS photo_no_delete BEFORE DELETE ON photo
  BEGIN SELECT RAISE(ABORT, '進貨照片不可刪除'); END;
CREATE TRIGGER IF NOT EXISTS audit_no_update BEFORE UPDATE ON audit
  BEGIN SELECT RAISE(ABORT, '操作紀錄不可修改'); END;
CREATE TRIGGER IF NOT EXISTS audit_no_delete BEFORE DELETE ON audit
  BEGIN SELECT RAISE(ABORT, '操作紀錄不可刪除'); END;
-- SKU 編碼由貨號、顏色代碼、尺寸代碼組成，組成元素建立後不可改
CREATE TRIGGER IF NOT EXISTS product_code_locked BEFORE UPDATE OF code, code_date, code_serial ON product
  BEGIN SELECT RAISE(ABORT, '本店貨號建立後不可修改'); END;
CREATE TRIGGER IF NOT EXISTS variant_locked BEFORE UPDATE OF product_id, color_id, size_id, sku ON variant
  BEGIN SELECT RAISE(ABORT, 'SKU 的商品、顏色、尺寸建立後不可修改'); END;
CREATE TRIGGER IF NOT EXISTS color_code_locked BEFORE UPDATE OF code ON color
  BEGIN SELECT RAISE(ABORT, '顏色代碼建立後不可修改'); END;
CREATE TRIGGER IF NOT EXISTS size_code_locked BEFORE UPDATE OF code ON size
  BEGIN SELECT RAISE(ABORT, '尺寸代碼建立後不可修改'); END;
"#;

const COLORS: [(&str, &str); 12] = [
    ("BK", "黑"), ("WH", "白"), ("GY", "灰"), ("NV", "深藍"), ("BL", "藍"), ("BE", "米"),
    ("KH", "卡其"), ("BR", "咖啡"), ("RD", "紅"), ("PK", "粉"), ("GN", "綠"), ("YL", "黃"),
];
const SIZES: [&str; 8] = ["XS", "S", "M", "L", "XL", "2XL", "3XL", "F"];
const KEEP_BACKUPS: usize = 30;

pub fn open(path: &Path) -> Result<Connection> {
    let c = Connection::open(path)?;
    c.busy_timeout(std::time::Duration::from_secs(5))?;
    c.pragma_update(None, "foreign_keys", "ON")?;
    c.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))?;
    c.pragma_update(None, "synchronous", "FULL")?; // 每次寫入都確實落盤，停電也不掉資料
    c.execute_batch(SCHEMA)?;
    let ver: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if ver == 0 {
        let tx = c.unchecked_transaction()?;
        for (code, name) in COLORS {
            tx.execute("INSERT OR IGNORE INTO color(code, name) VALUES(?1, ?2)", params![code, name])?;
        }
        for (i, code) in SIZES.iter().enumerate() {
            tx.execute("INSERT OR IGNORE INTO size(code, sort) VALUES(?1, ?2)", params![code, i as i64 * 10])?;
        }
        tx.pragma_update(None, "user_version", 1)?;
        log(&tx, "建立資料庫", "", "預載 12 種顏色、8 種尺寸")?;
        tx.commit()?;
    }
    Ok(c)
}

/// 每天第一次啟動時備份一份，保留最近 30 份。
pub fn backup(c: &Connection, dir: &Path) -> Result<String> {
    std::fs::create_dir_all(dir)?;
    let name = format!("備份-{}.sqlite3", Local::now().format("%Y%m%d"));
    let path = dir.join(&name);
    if !path.exists() {
        c.execute("VACUUM INTO ?1", [path.to_string_lossy()])?;
    }
    let mut all: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("備份-")))
        .collect();
    all.sort();
    for p in all.iter().rev().skip(KEEP_BACKUPS) {
        let _ = std::fs::remove_file(p);
    }
    Ok(name)
}

/// 把 SQLite 錯誤轉成店家看得懂的話。
pub fn message(e: &rusqlite::Error) -> String {
    if let rusqlite::Error::SqliteFailure(_, Some(m)) = e
        && !m.is_ascii()
    {
        return m.clone(); // 觸發器的中文訊息
    }
    let s = e.to_string();
    for (pat, msg) in [
        ("supplier.code", "廠商代號已存在，請換一個"),
        ("color.code", "顏色代碼已存在"),
        ("size.code", "尺寸代碼已存在"),
        ("variant.barcode", "這個條碼已被其他 SKU 使用"),
        ("FOREIGN KEY", "這筆資料已被商品或單據使用，無法刪除"),
        ("database is locked", "資料庫忙碌中，請稍後再試"),
        ("CHECK constraint", "資料內容不符規則（例如數量為 0 或金額為負數）"),
    ] {
        if s.contains(pat) {
            return msg.into();
        }
    }
    format!("資料庫錯誤：{s}")
}

pub fn log(c: &Connection, action: &str, target: &str, detail: &str) -> Result<()> {
    c.execute(
        "INSERT INTO audit(at, action, target, detail) VALUES(?1, ?2, ?3, ?4)",
        params![now(), action, target, detail],
    )?;
    Ok(())
}

pub fn now() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

pub fn today() -> NaiveDate {
    Local::now().date_naive()
}

pub fn parse_date(s: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| Error(format!("日期格式錯誤：{s}")))
}

fn val(v: ValueRef) -> Value {
    match v {
        ValueRef::Null | ValueRef::Blob(_) => Value::Null,
        ValueRef::Integer(i) => i.into(),
        ValueRef::Real(f) => f.into(),
        ValueRef::Text(t) => String::from_utf8_lossy(t).into(),
    }
}

/// 查詢結果轉成 JSON 陣列（每列一個陣列）。
pub fn rows<P: Params>(c: &Connection, sql: &str, p: P) -> Result<Vec<Vec<Value>>> {
    let mut st = c.prepare(sql)?;
    let n = st.column_count();
    let out = st
        .query_map(p, |r| (0..n).map(|i| r.get_ref(i).map(val)).collect())?
        .collect::<rusqlite::Result<_>>()?;
    Ok(out)
}

/// 查詢結果轉成 JSON 物件陣列（欄位名稱為 key）。
pub fn objects<P: Params>(c: &Connection, sql: &str, p: P) -> Result<Vec<Value>> {
    let names: Vec<String> = c.prepare(sql)?.column_names().into_iter().map(String::from).collect();
    Ok(rows(c, sql, p)?
        .into_iter()
        .map(|r| Value::Object(names.iter().cloned().zip(r).collect::<Map<_, _>>()))
        .collect())
}

pub fn object<P: Params>(c: &Connection, sql: &str, p: P) -> Result<Value> {
    match objects(c, sql, p)?.into_iter().next() {
        Some(v) => Ok(v),
        None => bail("找不到這筆資料，可能已被刪除"),
    }
}
