#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use clothes_stock::{Result, bail, db, history, media, reports, store};
use rusqlite::Connection;
use serde_json::{Value, json};
use tauri::State;

struct App {
    db: Mutex<Connection>,
    dir: PathBuf,
    status: Value,
}

impl App {
    fn db(&self) -> MutexGuard<'_, Connection> {
        self.db.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn media(&self) -> PathBuf {
        self.dir.join("圖片")
    }
    fn reports(&self) -> PathBuf {
        self.dir.join("報表")
    }
}

/// 資料位置：環境變數 SALES_DATA_DIR ＞ exe 旁已有資料庫（免安裝版）＞ %LOCALAPPDATA%\服飾庫存系統-資料（安裝版）。
/// 不放在程式資料夾，也不用 Tauri 的識別碼資料夾（解除安裝程式的「刪除應用程式資料」會清掉它），資料才不會被誤刪。
fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("SALES_DATA_DIR") {
        return dir.into();
    }
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_default();
    if exe_dir.join(DB_FILE).exists() {
        return exe_dir;
    }
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("服飾庫存系統-資料")).unwrap_or(exe_dir)
}

const DB_FILE: &str = "庫存資料.sqlite3";

fn explorer(arg: String) {
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new("explorer").raw_arg(arg).spawn();
}

fn message_box(msg: &str, kind: u32) -> i32 {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBoxW(hwnd: *mut std::ffi::c_void, text: *const u16, caption: *const u16, kind: u32) -> i32;
    }
    let w = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    unsafe { MessageBoxW(std::ptr::null_mut(), w(msg).as_ptr(), w("服飾庫存系統").as_ptr(), kind) }
}

fn alert(msg: &str) {
    message_box(msg, 0x10);
}

/// 啟動時到 GitHub Release 檢查新版；使用者同意後下載、安裝並重新啟動。
/// 檢查失敗（離線等）一律靜默略過，不影響正常使用。
async fn check_update(app: tauri::AppHandle) {
    use tauri_plugin_updater::UpdaterExt;
    let Ok(Some(update)) = async { app.updater()?.check().await }.await else { return };
    let ask = format!("有新版 v{}（目前 v{}），要現在更新嗎？

更新時程式會關閉，安裝完成後自動重新開啟，資料不受影響。", update.version, update.current_version);
    if message_box(&ask, 0x24 | 0x40000) != 6 {
        return;
    }
    match update.download_and_install(|_, _| {}, || {}).await {
        Ok(()) => app.restart(),
        Err(e) => alert(&format!("更新失敗，可稍後重開程式再試，或到 GitHub 下載安裝檔。

{e}")),
    }
}

#[tauri::command(async)]
fn meta(app: State<'_, App>) -> Result<Value> {
    Ok(json!({
        "categories": store::CATEGORIES, "payments": store::PAYMENTS, "kinds": store::KINDS, "tax_types": store::TAX_TYPES,
        "dir": app.dir.to_string_lossy(), "status": app.status,
    }))
}

#[tauri::command(async)]
fn dashboard(app: State<'_, App>) -> Result<Value> {
    store::dashboard(&app.db())
}

#[tauri::command(async)]
fn suppliers(app: State<'_, App>) -> Result<Vec<Value>> {
    store::suppliers(&app.db())
}

#[tauri::command(async)]
fn save_supplier(app: State<'_, App>, input: store::Supplier) -> Result<i64> {
    store::save_supplier(&mut app.db(), input)
}

#[tauri::command(async)]
fn colors(app: State<'_, App>) -> Result<Vec<Value>> {
    store::colors(&app.db())
}

#[tauri::command(async)]
fn sizes(app: State<'_, App>) -> Result<Vec<Value>> {
    store::sizes(&app.db())
}

#[tauri::command(async)]
fn save_color(app: State<'_, App>, input: store::Spec) -> Result<i64> {
    store::save_color(&mut app.db(), input)
}

#[tauri::command(async)]
fn save_size(app: State<'_, App>, input: store::Spec) -> Result<i64> {
    store::save_size(&mut app.db(), input)
}

#[tauri::command(async)]
fn delete_item(app: State<'_, App>, what: String, id: i64) -> Result<()> {
    store::delete(&mut app.db(), &what, id)
}

#[tauri::command(async)]
fn products(app: State<'_, App>, q: String) -> Result<Vec<Value>> {
    store::products(&app.db(), &q)
}

#[tauri::command(async)]
fn product(app: State<'_, App>, id: i64) -> Result<Value> {
    store::product(&app.db(), id)
}

#[tauri::command(async)]
fn save_product(app: State<'_, App>, input: store::ProductIn) -> Result<i64> {
    store::save_product(&mut app.db(), input)
}

#[tauri::command(async)]
fn add_variants(app: State<'_, App>, id: i64, colors: Vec<i64>, sizes: Vec<i64>) -> Result<()> {
    store::add_variants(&mut app.db(), id, &colors, &sizes)
}

#[tauri::command(async)]
fn update_variant(app: State<'_, App>, id: i64, safety: i64) -> Result<()> {
    store::update_variant(&mut app.db(), id, safety)
}

#[tauri::command(async)]
fn find(app: State<'_, App>, q: String) -> Result<Value> {
    store::find(&app.db(), &q)
}

/// 前端以原始位元組上傳圖片，標頭 x-kind 指定 products 或 purchases。
#[tauri::command(async)]
fn save_image(app: State<'_, App>, request: tauri::ipc::Request<'_>) -> Result<String> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else { return bail("圖片上傳格式錯誤") };
    let kind = request.headers().get("x-kind").and_then(|v| v.to_str().ok()).unwrap_or("");
    media::save_upload(&app.media(), kind, bytes)
}

#[tauri::command(async)]
fn create_doc(app: State<'_, App>, input: store::DocIn) -> Result<String> {
    store::create_doc(&mut app.db(), input)
}

#[tauri::command(async)]
fn docs(app: State<'_, App>, filter: store::DocFilter) -> Result<Vec<Value>> {
    store::docs(&app.db(), filter)
}

#[tauri::command(async)]
fn doc(app: State<'_, App>, id: i64) -> Result<Value> {
    store::doc(&app.db(), id)
}

#[tauri::command(async)]
fn void_doc(app: State<'_, App>, id: i64, reason: String) -> Result<()> {
    store::void_doc(&mut app.db(), id, &reason)
}

#[tauri::command(async)]
fn report(app: State<'_, App>, name: String, params: reports::Params) -> Result<reports::Report> {
    reports::build(&app.db(), &name, &params)
}

#[tauri::command(async)]
fn export_report(app: State<'_, App>, name: String, params: reports::Params) -> Result<String> {
    let path = reports::export(&app.db(), &app.reports(), &app.media(), &name, &params)?;
    explorer(format!("/select,\"{}\"", path.display()));
    Ok(path.to_string_lossy().into())
}

#[tauri::command(async)]
fn export_three(app: State<'_, App>) -> Result<String> {
    let path = reports::export_three(&app.db(), &app.reports(), &app.media())?;
    explorer(format!("/select,\"{}\"", path.display()));
    Ok(path.to_string_lossy().into())
}

#[tauri::command(async)]
fn export_sale(app: State<'_, App>, id: i64) -> Result<String> {
    let path = reports::export_sale(&app.db(), &app.reports(), id)?;
    explorer(format!("/select,\"{}\"", path.display()));
    Ok(path.to_string_lossy().into())
}

/// 前端以原始位元組上傳 Excel，標頭 x-name 為 encodeURIComponent 後的檔名。
#[tauri::command(async)]
fn import_history(app: State<'_, App>, request: tauri::ipc::Request<'_>) -> Result<Value> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else { return bail("檔案上傳格式錯誤") };
    let name = request.headers().get("x-name").and_then(|v| v.to_str().ok()).map(percent_decode).unwrap_or_default();
    history::import(&mut app.db(), &name, bytes)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let (mut out, mut i) = (Vec::with_capacity(b.len()), 0);
    while i < b.len() {
        match (b[i], b.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok())) {
            (b'%', Some(v)) => { out.push(v); i += 3; }
            (c, _) => { out.push(c); i += 1; }
        }
    }
    String::from_utf8_lossy(&out).into()
}

#[tauri::command(async)]
fn histories(app: State<'_, App>, start: String, end: String, q: String) -> Result<Vec<Value>> {
    history::list(&app.db(), &start, &end, &q)
}

#[tauri::command(async)]
fn history_doc(app: State<'_, App>, id: i64) -> Result<Value> {
    history::get(&app.db(), id)
}

#[tauri::command(async)]
fn delete_history(app: State<'_, App>, id: i64) -> Result<()> {
    history::delete(&mut app.db(), id)
}

#[tauri::command(async)]
fn audit(app: State<'_, App>, q: String) -> Result<Vec<Value>> {
    store::audit(&app.db(), &q)
}

#[tauri::command(async)]
fn recalc_all(app: State<'_, App>) -> Result<String> {
    store::recalc_all(&mut app.db())
}

#[tauri::command(async)]
fn open_folder(app: State<'_, App>, which: String) -> Result<()> {
    let dir = match which.as_str() {
        "reports" => app.reports(),
        "backup" => app.dir.join("備份"),
        _ => app.dir.clone(),
    };
    std::fs::create_dir_all(&dir)?;
    explorer(format!("\"{}\"", dir.display()));
    Ok(())
}

fn start(dir: &Path) -> Result<(Connection, Value)> {
    std::fs::create_dir_all(dir)?;
    let c = db::open(&dir.join(DB_FILE))?;
    let check: String = c.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    let backup = db::backup(&c, &dir.join("備份"))?;
    Ok((c, json!({"quick_check": check, "backup": backup})))
}

fn main() {
    let dir = data_dir();
    let (conn, status) = match start(&dir) {
        Ok(v) => v,
        Err(e) => {
            alert(&format!("無法開啟資料庫，系統未啟動。\n\n{}\n\n資料位置：{}", e.0, dir.display()));
            std::process::exit(1);
        }
    };
    let media_dir = dir.join("圖片");
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            tauri::async_runtime::spawn(check_update(app.handle().clone()));
            Ok(())
        })
        .manage(App { db: Mutex::new(conn), dir, status })
        // 商品圖與進貨照片：http://media.localhost/<相對路徑>
        .register_uri_scheme_protocol("media", move |_ctx, req| {
            let rel = req.uri().path().trim_start_matches('/').to_string();
            let safe = !rel.is_empty() && !rel.split('/').any(|p| p.is_empty() || p == "..");
            match safe.then(|| std::fs::read(media_dir.join(&rel)).ok()).flatten() {
                Some(bytes) => tauri::http::Response::builder()
                    .header("Content-Type", "image/jpeg")
                    .header("Cache-Control", "max-age=31536000")
                    .body(bytes)
                    .unwrap_or_default(),
                None => tauri::http::Response::builder().status(404).body(Vec::new()).unwrap_or_default(),
            }
        })
        .invoke_handler(tauri::generate_handler![
            meta, dashboard, suppliers, save_supplier, colors, sizes, save_color, save_size, delete_item,
            products, product, save_product, add_variants, update_variant, find, save_image,
            create_doc, docs, doc, void_doc, report, export_report, export_three, export_sale, import_history, histories, history_doc, delete_history, audit, recalc_all, open_folder,
        ])
        .run(tauri::generate_context!())
        .unwrap_or_else(|e| alert(&format!("系統發生錯誤：{e}")));
}
