'use strict';
const { invoke } = window.__TAURI__.core;
const MEDIA = 'http://media.localhost/';

// ---------- 工具 ----------
const $ = (s, el = document) => el.querySelector(s);
const $$ = (s, el = document) => [...el.querySelectorAll(s)];
const esc = v => String(v ?? '').replace(/[&<>"']/g, c => `&#${c.charCodeAt(0)};`);
const money = c => (c < 0 ? '-' : '') + '$' + (Math.abs(c || 0) / 100).toLocaleString('zh-TW', { maximumFractionDigits: 2 });
const dollars = c => (c || 0) / 100;
const cents = v => (v === '' || v == null) ? NaN : Math.round(parseFloat(v) * 100);
const today = () => new Date().toLocaleDateString('sv');
const monthStart = () => today().slice(0, 8) + '01';
const img = (p, cls = 'thumb') => p ? `<img class="${cls}" src="${MEDIA}${esc(p)}" alt="" loading="lazy">` : `<span class="${cls} noimg"></span>`;
const options = (list, cur) => list.map(v => `<option ${v === cur ? 'selected' : ''}>${esc(v)}</option>`).join('');
const TYPE = { SO: '銷貨', PO: '進貨', AJ: '調整' };
const drafts = {};
let META = {};

async function call(cmd, args, opts) {
  try { return await invoke(cmd, args, opts); }
  catch (e) { toast(String(e), 'error'); throw e; }
}

function toast(msg, type = 'ok') {
  const t = document.createElement('div');
  t.className = `toast ${type}`;
  t.textContent = msg;
  t.setAttribute('role', type === 'error' ? 'alert' : 'status');
  t.onclick = () => t.remove();
  $('#toasts').append(t);
  setTimeout(() => t.remove(), type === 'error' ? 9000 : 3500);
}

function head(title, sub = '', actions = '') {
  return `<header class="page-head"><div><h1>${title}</h1>${sub ? `<p class="muted">${sub}</p>` : ''}</div>
    <div class="head-actions">${actions}</div></header>`;
}

/** heads: [文字 或 [文字, 'num']]；rows: 每列為儲存格 HTML 陣列，或 {cells, attrs} */
function table(heads, rows, empty = '沒有資料') {
  if (!rows.length) return `<div class="empty">${empty}</div>`;
  const cls = h => Array.isArray(h) && h[1] === 'num' ? ' class="num"' : '';
  return `<div class="table-wrap"><table><thead><tr>${heads.map(h => `<th${cls(h)}>${esc(Array.isArray(h) ? h[0] : h)}</th>`).join('')}</tr></thead>
    <tbody>${rows.map(r => {
      const cells = Array.isArray(r) ? r : r.cells;
      return `<tr ${Array.isArray(r) ? '' : r.attrs}>${cells.map((c, i) => `<td${cls(heads[i])}>${c ?? ''}</td>`).join('')}</tr>`;
    }).join('')}</tbody></table></div>`;
}

function modal(html, wide) {
  const d = document.createElement('dialog');
  if (wide) d.className = 'wide';
  d.innerHTML = html;
  document.body.append(d);
  d.addEventListener('close', () => d.remove());
  d.showModal();
  return d;
}

function confirmBox(title, body, ok = '確定', danger = false) {
  const d = modal(`<h2>${esc(title)}</h2><div class="dlg-body">${body}</div>
    <div class="actions"><button class="btn" value="">取消</button><button class="btn ${danger ? 'danger' : 'primary'}" value="1">${esc(ok)}</button></div>`);
  $$('.actions .btn', d)[1].focus();
  return new Promise(res => {
    $$('.actions .btn', d).forEach(b => b.onclick = () => { d.close(); res(!!b.value); });
    d.addEventListener('cancel', () => res(false));
  });
}

function promptBox(title, label, ok = '確定', danger = false) {
  const d = modal(`<h2>${esc(title)}</h2><label style="width:100%">${esc(label)}<input id="pb-in" style="width:100%"></label>
    <p class="err"></p><div class="actions"><button class="btn" value="">取消</button><button class="btn ${danger ? 'danger' : 'primary'}" value="1">${esc(ok)}</button></div>`);
  const input = $('#pb-in', d);
  input.focus();
  return new Promise(res => {
    const done = v => { d.close(); res(v); };
    $$('.actions .btn', d)[0].onclick = () => done(null);
    $$('.actions .btn', d)[1].onclick = () => input.value.trim() ? done(input.value.trim()) : ($('.err', d).textContent = `請填寫${label}`);
    input.onkeydown = e => { if (e.key === 'Enter') $$('.actions .btn', d)[1].click(); };
    d.addEventListener('cancel', () => res(null));
  });
}

function showPhoto(p) {
  const d = modal(`<img class="photo-big" src="${MEDIA}${esc(p)}" alt=""><div class="actions"><button class="btn">關閉</button></div>`, true);
  $('.btn', d).onclick = () => d.close();
}

async function uploadImage(file, kind) {
  const buf = new Uint8Array(await file.arrayBuffer());
  return call('save_image', buf, { headers: { 'x-kind': kind } });
}

function stat(label, value, sub) {
  return `<div class="stat"><div class="label">${label}</div><div class="value">${value}</div><div class="sub">${sub}</div></div>`;
}

function docSummary(d) {
  if (d.type === 'PO') return esc(d.supplier);
  if (d.type === 'SO') return esc([d.customer_name, d.payment, d.tax_type].filter(Boolean).join('・'));
  return esc(`${d.kind}：${d.reason}`);
}

function docRows(list) {
  return list.map(d => ({
    attrs: `data-doc="${d.id}" class="${d.status === 'void' ? 'void' : ''}"`,
    cells: [`<b>${esc(d.no)}</b>`, esc(d.date), `<span class="tag ${d.type}">${TYPE[d.type]}</span>`, docSummary(d),
      d.qty, d.type === 'AJ' ? '' : money(d.total),
      d.status === 'void' ? '<span class="tag void">作廢</span>' : '<span class="tag">已過帳</span>'],
  }));
}
const DOC_HEADS = ['單號', '日期', '類型', '內容', ['件數', 'num'], ['金額', 'num'], '狀態'];

// ---------- 掃碼／搜尋框 ----------
function scanBox(id, placeholder) {
  return `<div class="scan"><div class="row"><input type="search" id="${id}" placeholder="${esc(placeholder)}" autocomplete="off">
    <button class="btn" data-scan="${id}">搜尋</button></div><div class="scan-results" id="${id}-r"></div></div>`;
}

/** onVariant(v)：SKU 完全相符；onProduct(p)：選定商品（含 variants） */
function bindScan(id, { onVariant, onProduct }) {
  const input = $('#' + id), box = $(`#${id}-r`);
  const go = async () => {
    const q = input.value.trim();
    box.innerHTML = '';
    if (!q) return;
    const r = await call('find', { q });
    if (r.variant && onVariant) { input.value = ''; return onVariant(r.variant, r); }
    if (r.variant) r.products = r.products.filter(p => p.id === r.variant.product_id);
    if (r.products.length === 1) { input.value = ''; return onProduct(r.products[0]); }
    if (!r.products.length) {
      box.innerHTML = `<div class="msg">找不到「${esc(q)}」。請確認貨號或品名，或先到「商品」建檔。</div>`;
      return;
    }
    box.innerHTML = r.products.map((p, i) => `<button data-i="${i}">${img(p.image)}<span><b>${esc(p.code)}</b> ${esc(p.name)}<br>
      <small class="muted">${esc(p.category)}・售價 ${money(p.list_price)}・庫存 ${p.stock}</small></span></button>`).join('');
    $$('button', box).forEach(b => b.onclick = () => { box.innerHTML = ''; input.value = ''; onProduct(r.products[b.dataset.i]); });
    $('button', box).focus();
  };
  input.onkeydown = e => {
    if (e.key === 'Enter') { e.preventDefault(); go(); }
    if (e.key === 'Escape') box.innerHTML = '';
    if (e.key === 'ArrowDown') $('button', box)?.focus();
  };
  $(`[data-scan="${id}"]`).onclick = go;
  input.focus();
}

// ---------- 顏色 × 尺寸矩陣 ----------
function matrixParts(p) {
  const vs = p.variants || [];
  const sizes = [...new Map(vs.map(v => [v.size_id, v])).values()].sort((a, b) => a.size_sort - b.size_sort || a.size.localeCompare(b.size));
  const colors = [...new Map(vs.map(v => [v.color_id, v])).values()].sort((a, b) => a.color_code.localeCompare(b.color_code));
  const cell = new Map(vs.map(v => [`${v.color_id}-${v.size_id}`, v]));
  return { sizes, colors, cell };
}

function matrixTable(p, render) {
  const { sizes, colors, cell } = matrixParts(p);
  if (!colors.length) return '<div class="empty">這個商品還沒有 SKU，請先加顏色／尺寸。</div>';
  return `<div class="table-wrap"><table class="matrix"><thead><tr><th>顏色＼尺寸</th>${sizes.map(s => `<th>${esc(s.size)}</th>`).join('')}</tr></thead>
    <tbody>${colors.map(c => `<tr><th>${esc(c.color)}</th>${sizes.map(s => {
      const v = cell.get(`${c.color_id}-${s.size_id}`);
      return v ? render(v) : '<td class="zero">—</td>';
    }).join('')}</tr>`).join('')}</tbody></table></div>`;
}

const stockCell = v => `<td class="${v.qty_on_hand < v.safety_stock ? 'low' : v.qty_on_hand ? '' : 'zero'}" title="${esc(v.sku)}">${v.qty_on_hand}</td>`;

/** mode: sale｜purchase｜opening（期初庫存）回傳 [{variant, qty, price}]；pick 回傳單一 variant */
function pickMatrix(product, mode) {
  const incoming = mode === 'purchase' || mode === 'opening';
  const priceLabel = { sale: '售價', purchase: '進貨單價', opening: '單位成本' }[mode];
  return new Promise(resolve => {
    let p = product;
    const d = modal('', true);
    let result = null;
    d.addEventListener('close', () => resolve(result));
    const render = () => {
      const body = mode === 'pick'
        ? matrixTable(p, v => `<td><button class="cell" data-v="${v.id}">${v.qty_on_hand}<small>件</small></button></td>`)
        : matrixTable(p, v => `<td><input type="number" min="${mode === 'sale' ? '' : 0}" step="1" data-v="${v.id}" placeholder="0" aria-label="${esc(v.sku)}"><small>庫存 ${v.qty_on_hand}</small></td>`);
      d.innerHTML = `<h2>${esc(p.code)} ${esc(p.name)}</h2>
        <p class="muted">${mode === 'pick' ? '點選要調整的顏色尺寸' : '在格子裡填數量，空白表示不加入'}</p>
        <div class="dlg-body">${body}
        ${mode === 'pick' ? '' : `<div class="row" style="margin-top:14px"><label>${priceLabel}${incoming ? ' *' : ''}
          <input type="number" id="m-price" min="0" step="1" value="${mode === 'sale' ? dollars(p.list_price) : (drafts.lastCost?.[p.id] ?? '')}"></label>
          ${incoming ? '<button class="btn small" id="m-add" style="align-self:flex-end">＋ 加顏色／尺寸</button>' : ''}</div>`}
        <div id="m-addbox"></div></div>
        <p class="err"></p>
        <div class="actions"><button class="btn" id="m-cancel">取消</button>${mode === 'pick' ? '' : '<button class="btn primary" id="m-ok">加入明細</button>'}</div>`;
      $('#m-cancel', d).onclick = () => d.close();
      $$('button.cell', d).forEach(b => b.onclick = () => { result = p.variants.find(v => v.id === +b.dataset.v); d.close(); });
      $('input[data-v]', d)?.focus();
      $$('input[data-v]', d).forEach(i => i.onkeydown = e => { if (e.key === 'Enter') $('#m-ok', d).click(); });
      if ($('#m-add', d)) $('#m-add', d).onclick = addSpecs;
      if ($('#m-ok', d)) $('#m-ok', d).onclick = () => {
        const price = $('#m-price', d).value;
        const lines = $$('input[data-v]', d).filter(i => i.value !== '' && +i.value !== 0)
          .map(i => ({ variant: p.variants.find(v => v.id === +i.dataset.v), qty: +i.value, price }));
        const err = $('.err', d);
        if (!lines.length) return err.textContent = '請至少在一格填入數量';
        if (lines.some(l => !Number.isInteger(l.qty))) return err.textContent = '數量必須是整數';
        if (incoming && lines.some(l => l.qty < 0)) return err.textContent = '數量不可為負數';
        if (isNaN(cents(price)) || cents(price) < 0) return err.textContent = `請填寫${priceLabel}`;
        if (incoming) (drafts.lastCost ??= {})[p.id] = price;
        result = lines;
        d.close();
      };
    };
    const addSpecs = async () => {
      const [colors, sizes] = await Promise.all([call('colors'), call('sizes')]);
      $('#m-addbox', d).innerHTML = `<fieldset><legend>新增顏色</legend><div class="checks-grid">${chips('ac', colors, c => c.name)}</div></fieldset>
        <fieldset><legend>新增尺寸</legend><div class="checks-grid">${chips('as', sizes, s => s.code)}</div></fieldset>
        <div class="row" style="margin-top:10px"><span class="hint">勾選的顏色 × 尺寸會建立 SKU（已存在的略過）</span><button class="btn small" id="m-addok">建立 SKU</button></div>`;
      $('#m-addok', d).onclick = async () => {
        const colorIds = checked(d, 'ac'), sizeIds = checked(d, 'as');
        if (!colorIds.length || !sizeIds.length) return $('.err', d).textContent = '顏色與尺寸都要至少勾選一個';
        await call('add_variants', { id: p.id, colors: colorIds, sizes: sizeIds });
        p = await call('product', { id: p.id });
        toast('已建立 SKU');
        render();
      };
    };
    render();
  });
}

const chips = (name, list, label, isChecked = () => false) =>
  list.map(x => `<label class="chip"><input type="checkbox" name="${name}" value="${x.id}" ${isChecked(x) ? 'checked' : ''}>${esc(label(x))}</label>`).join('');
const checked = (el, name) => $$(`input[name="${name}"]:checked`, el).map(i => +i.value);

// ---------- 商品表單（商品頁與進貨快速建檔共用） ----------
async function productForm(p) {
  const [colors, sizes, suppliers] = await Promise.all([call('colors'), call('sizes'), call('suppliers')]);
  const isNew = !p.id;
  return `<div class="form-grid">
      <label>品名 *<input name="name" value="${esc(p.name)}" required></label>
      <label>類別<select name="category">${options(META.categories, p.category || META.categories[0])}</select></label>
      <label>預設廠商<select name="supplier_id"><option value="">（不指定）</option>
        ${suppliers.map(s => `<option value="${s.id}" ${s.id === p.supplier_id ? 'selected' : ''}>${esc(s.code)} ${esc(s.name)}</option>`).join('')}</select></label>
      <label>廠商貨號<input name="supplier_code" value="${esc(p.supplier_code)}"></label>
      <label>建議售價<input type="number" name="list_price" min="0" step="1" value="${p.id ? dollars(p.list_price) : ''}" placeholder="0" style="width:100%"></label>
      <label>備註<input name="note" value="${esc(p.note)}"></label>
    </div>
    <div class="image-field"><span class="pf-preview">${img(p.image, 'preview')}</span>
      <label class="btn">選擇商品主圖<input type="file" accept="image/*" name="imagefile" hidden></label>
      ${p.image ? '<button class="btn small" type="button" data-noimg>移除圖片</button>' : ''}
      <input type="hidden" name="image" value="${esc(p.image)}"></div>
    <fieldset><legend>${isNew ? '顏色（勾選後自動建立 SKU）' : '新增顏色（已存在的組合會略過）'}</legend>
      <div class="checks-grid">${chips('pc', colors, c => c.name)}</div></fieldset>
    <fieldset><legend>${isNew ? '尺寸' : '新增尺寸'}</legend><div class="checks-grid">${chips('ps', sizes, s => s.code)}</div></fieldset>`;
}

function bindProductForm(el) {
  $('[name=imagefile]', el).onchange = async e => {
    const f = e.target.files[0];
    if (!f) return;
    const path = await uploadImage(f, 'products');
    $('[name=image]', el).value = path;
    $('.pf-preview', el).innerHTML = img(path, 'preview');
  };
  const rm = $('[data-noimg]', el);
  if (rm) rm.onclick = () => { $('[name=image]', el).value = ''; $('.pf-preview', el).innerHTML = img('', 'preview'); };
}

function readProductForm(el, id) {
  const v = n => $(`[name=${n}]`, el).value;
  return {
    id: id ?? null, name: v('name'), category: v('category'), supplier_id: +v('supplier_id') || null,
    supplier_code: v('supplier_code'), list_price: cents(v('list_price')) || 0, image: v('image'), note: v('note'),
    color_ids: checked(el, 'pc'), size_ids: checked(el, 'ps'),
  };
}

/** 快速建檔對話框，成功回傳完整商品 */
async function quickProduct(defaults) {
  const d = modal(`<h2>新商品建檔</h2><form class="dlg-body" id="qp">${await productForm(defaults)}</form><p class="err"></p>
    <div class="actions"><button class="btn" id="qp-cancel">取消</button><button class="btn primary" id="qp-ok">建立商品</button></div>`, true);
  bindProductForm(d);
  $('[name=name]', d).focus();
  return new Promise(res => {
    d.addEventListener('close', () => res(null));
    $('#qp-cancel', d).onclick = () => d.close();
    $('#qp-ok', d).onclick = async () => {
      const input = readProductForm(d);
      if (!input.name.trim()) return $('.err', d).textContent = '請填寫品名';
      if (!input.color_ids.length || !input.size_ids.length) return $('.err', d).textContent = '請至少勾選一個顏色和一個尺寸';
      const id = await call('save_product', { input });
      const p = await call('product', { id });
      toast(`已建立商品 ${p.code}`);
      d.onclose = null;
      res(p);
      d.close();
    };
  });
}

async function supplierDialog(s = {}) {
  const f = (n, label, req) => `<label>${label}${req ? ' *' : ''}<input name="${n}" value="${esc(s[n])}"></label>`;
  const d = modal(`<h2>${s.id ? '修改廠商' : '新增廠商'}</h2><div class="form-grid">${f('code', '廠商代號', 1)}${f('name', '廠商名稱', 1)}
    ${f('contact', '聯絡人')}${f('phone', '電話')}</div><div class="form-grid" style="margin-top:14px">${f('address', '地址')}${f('note', '備註')}</div>
    <p class="err"></p><div class="actions">${s.id && !s.used ? '<button class="btn danger" id="sd-del" style="margin-right:auto">刪除</button>' : ''}
    <button class="btn" id="sd-cancel">取消</button><button class="btn primary" id="sd-ok">儲存</button></div>`);
  $('[name=code]', d).focus();
  return new Promise(res => {
    d.addEventListener('close', () => res(null));
    $('#sd-cancel', d).onclick = () => d.close();
    if ($('#sd-del', d)) $('#sd-del', d).onclick = async () => {
      if (!await confirmBox('刪除廠商', `確定刪除「${esc(s.name)}」？`, '刪除', true)) return;
      await call('delete_item', { what: 'supplier', id: s.id });
      toast('已刪除廠商');
      d.onclose = null; res('deleted'); d.close();
    };
    $('#sd-ok', d).onclick = async () => {
      const input = Object.fromEntries($$('input', d).map(i => [i.name, i.value]));
      if (!input.code.trim() || !input.name.trim()) return $('.err', d).textContent = '請填寫廠商代號與名稱';
      const id = await call('save_supplier', { input: { ...input, id: s.id ?? null } });
      toast('已儲存廠商');
      d.onclose = null; res(id); d.close();
    };
  });
}

// ---------- 單據明細對話框 ----------
async function showDoc(id) {
  const d = await call('doc', { id });
  const isVoid = d.status === 'void';
  const total = d.lines.reduce((a, l) => a + l.subtotal, 0);
  const info = [
    ['日期', d.date], ['類型', TYPE[d.type]],
    d.type === 'PO' && ['廠商', d.supplier], d.type === 'SO' && ['付款', d.payment], d.type === 'SO' && ['課稅別', d.tax_type],
    d.customer_name && ['客戶', d.customer_name], d.customer_phone && ['電話', d.customer_phone], d.customer_address && ['地址', d.customer_address],
    d.type === 'AJ' && ['類型', d.kind], d.type === 'AJ' && ['原因', d.reason],
    d.note && ['備註', d.note], ['建檔時間', d.created_at],
    isVoid && ['作廢時間', d.voided_at], isVoid && ['作廢原因', d.void_reason],
  ].filter(Boolean);
  const priced = d.type !== 'AJ' || d.kind === '期初庫存';
  const heads = priced ? ['', 'SKU', '品名', '規格', ['數量', 'num'], [d.type === 'AJ' ? '單位成本' : '單價', 'num'], ['小計', 'num']]
    : ['', 'SKU', '品名', '規格', ['數量', 'num']];
  const rows = d.lines.map(l => [img(l.image), esc(l.sku), esc(l.name), esc(`${l.color}/${l.size}`), l.qty,
    ...(priced ? [money(l.unit_price), money(l.subtotal)] : [])]);
  const dlg = modal(`<h2>${TYPE[d.type]}單 ${esc(d.no)} ${isVoid ? '<span class="tag void">已作廢</span>' : ''}</h2>
    <div class="dlg-body"><p class="kv">${info.map(([k, v]) => `<span>${k}：<b>${esc(v)}</b></span>`).join('')}</p>
    ${d.photos.length ? `<div class="photos">${d.photos.map(p => `<div class="ph"><img src="${MEDIA}${esc(p.image)}" data-photo="${esc(p.image)}" alt=""></div>`).join('')}</div>` : ''}
    ${table(heads, rows)}
    ${!priced ? '' : `<p style="text-align:right">${d.discount || d.tax ? `小計 ${money(total)}　` : ''}${d.discount ? `折扣 ${money(d.discount)}　` : ''}${d.tax ? `營業稅 ${money(d.tax)}　` : ''}<b class="big-total">${money(total - d.discount + d.tax)}</b></p>`}
    </div><div class="actions">${isVoid ? '' : '<button class="btn danger" id="dd-void" style="margin-right:auto">作廢這張單據</button>'}
    ${d.type === 'SO' ? '<button class="btn" id="dd-export">匯出銷貨單 Excel</button>' : ''}<button class="btn" id="dd-close">關閉</button></div>`, true);
  $('#dd-close', dlg).onclick = () => dlg.close();
  $('#dd-export', dlg)?.addEventListener('click', async () => toast(`已匯出：${await call('export_sale', { id })}`));
  $('#dd-void', dlg)?.addEventListener('click', async () => {
    const reason = await promptBox(`作廢 ${d.no}`, '作廢原因', '確定作廢', true);
    if (!reason) return;
    await call('void_doc', { id, reason });
    toast(`已作廢 ${d.no}，庫存與成本已重算`);
    dlg.close();
    route();
  });
}

// ---------- 頁面 ----------
const PAGES = {};

PAGES.home = async main => {
  const d = await call('dashboard');
  const s = META.status;
  main.innerHTML = head('首頁', new Date().toLocaleDateString('zh-TW', { dateStyle: 'full' })) + `
    <div class="quick">
      <a class="big-btn" href="#/sale"><b>銷貨收銀</b><span>開單、退貨</span></a>
      <a class="big-btn" href="#/purchase"><b>進貨</b><span>廠商到貨入庫</span></a>
      <a class="big-btn" href="#/lookup"><b>查庫存</b><span>貨號、SKU、品名</span></a>
      <a class="big-btn" href="#/product/new"><b>新增商品</b><span>建檔並產生 SKU</span></a>
    </div>
    <div class="stats">
      ${stat('今日銷貨', money(d.today_sales), `${d.today_count} 張銷貨單`)}
      ${stat('本月銷貨', money(d.month_sales), `毛利 ${money(d.month_profit)}`)}
      ${stat('目前庫存', `${d.stock_qty.toLocaleString()} 件`, `成本 ${money(d.stock_value)}`)}
      ${stat('低庫存', `${d.low.length} 項`, '低於安全庫存的 SKU')}
    </div>
    <div class="grid2">
      <section class="card"><h2>低於安全庫存</h2>${table(['SKU', '品名', '規格', ['庫存', 'num'], ['安全庫存', 'num']],
        d.low.map(v => [esc(v.sku), esc(v.name), esc(`${v.color}/${v.size}`), `<span class="red">${v.qty_on_hand}</span>`, v.safety_stock]),
        '目前沒有低庫存的商品')}</section>
      <section class="card"><h2>最近單據</h2>${table(DOC_HEADS, docRows(d.recent), '還沒有任何單據')}</section>
    </div>
    <section class="card"><h2>資料安全</h2><ul class="checks">
      <li class="${s.quick_check === 'ok' ? 'ok' : 'bad'}">資料庫檔案檢查：${s.quick_check === 'ok' ? '正常' : esc(s.quick_check)}</li>
      <li class="${d.mismatch.length ? 'bad' : 'ok'}">庫存帳核對：${d.mismatch.length
        ? `${d.mismatch.length} 個 SKU 的庫存與單據不一致（${d.mismatch.slice(0, 5).map(m => esc(m.sku)).join('、')}） <button class="btn small" id="fix">重新核算</button>`
        : '每個 SKU 的庫存都與單據加總相符'}</li>
      <li class="ok">今日自動備份：${esc(s.backup)}（保留最近 30 份）</li>
      </ul>
      <div class="row"><span class="muted">資料位置：${esc(META.dir)}</span>
        <button class="btn small" data-open="data">開啟資料夾</button><button class="btn small" data-open="backup">開啟備份資料夾</button></div>
    </section>`;
  $('#fix')?.addEventListener('click', async () => { toast(await call('recalc_all')); route(); });
};

// ---- 銷貨 ----
PAGES.sale = async main => {
  const s = drafts.sale ??= { date: today(), payment: META.payments[0], tax_type: META.tax_types[0], discount: '', note: '',
    customer_name: '', customer_phone: '', customer_address: '', lines: [] };
  main.innerHTML = head('銷貨收銀', '輸入貨號、SKU 或品名後按 Enter。客人退貨：把數量改成負數。',
      '<button class="btn small" id="s-clear">清空重來</button>') + `
    <section class="card"><div class="toolbar">
      <label>日期<input type="date" id="s-date" value="${s.date}" max="${today()}"></label>
      <label>付款方式<select id="s-payment">${options(META.payments, s.payment)}</select></label>
      <label>課稅別<select id="s-tax_type">${options(META.tax_types, s.tax_type)}</select></label>
      ${scanBox('s-scan', '輸入貨號、SKU 或品名')}
    </div>
    <details class="customer" ${s.customer_name || s.customer_phone || s.customer_address ? 'open' : ''}>
      <summary>客戶資料（選填，會印在出貨單上）</summary>
      <div class="toolbar">
        <label>客戶名稱<input id="s-customer_name" value="${esc(s.customer_name)}"></label>
        <label>電話<input id="s-customer_phone" value="${esc(s.customer_phone)}"></label>
        <label class="grow">地址<input id="s-customer_address" value="${esc(s.customer_address)}" style="width:100%"></label>
      </div>
    </details></section>
    <section class="card"><h2>銷貨明細</h2><div id="s-lines"></div></section>
    <div class="checkout">
      <label>整單折扣<input type="number" id="s-discount" min="0" step="1" value="${esc(s.discount)}" placeholder="0"></label>
      <label class="grow">備註<input id="s-note" value="${esc(s.note)}" style="width:100%"></label>
      <div class="total"><span id="s-count"></span><b id="s-total"></b></div>
      <button class="btn primary big" id="s-submit">結帳</button>
    </div>`;
  for (const k of ['date', 'payment', 'tax_type', 'discount', 'note', 'customer_name', 'customer_phone', 'customer_address'])
    $(`#s-${k}`).oninput = e => { s[k] = e.target.value; saleTotals(); };
  $('#s-clear').onclick = async () => {
    if (s.lines.length && !await confirmBox('清空銷貨單', '目前的明細會全部清除，確定嗎？', '清空', true)) return;
    drafts.sale = null; route();
  };
  const add = (v, qty = 1, price) => {
    const l = s.lines.find(l => l.variant.id === v.id);
    if (l) l.qty += qty; else s.lines.push({ variant: v, qty, price: price ?? String(dollars(v.list_price)) });
    renderSaleLines();
  };
  bindScan('s-scan', {
    onVariant: v => add(v),
    onProduct: async p => { (await pickMatrix(p, 'sale'))?.forEach(l => add(l.variant, l.qty, l.price)); $('#s-scan').focus(); },
  });
  $('#s-submit').onclick = submitSale;
  renderSaleLines();
};

function saleNeed(s, vid) { return s.lines.filter(l => l.variant.id === vid).reduce((a, l) => a + (+l.qty || 0), 0); }

function renderSaleLines() {
  const s = drafts.sale, box = $('#s-lines');
  box.innerHTML = table(['', 'SKU／品名', '規格', ['庫存', 'num'], ['數量', 'num'], ['單價', 'num'], ['小計', 'num'], ''],
    s.lines.map((l, i) => {
      const short = saleNeed(s, l.variant.id) > l.variant.qty_on_hand;
      return { attrs: short ? 'class="short"' : '', cells: [img(l.variant.image), `<b>${esc(l.variant.sku)}</b><br>${esc(l.variant.name)}`,
        esc(`${l.variant.color}/${l.variant.size}`), short ? `<span class="red" title="庫存不足">${l.variant.qty_on_hand}</span>` : l.variant.qty_on_hand,
        `<input type="number" step="1" data-i="${i}" data-f="qty" value="${l.qty}" aria-label="數量">`,
        `<input type="number" min="0" step="1" data-i="${i}" data-f="price" value="${esc(l.price)}" aria-label="單價">`,
        `<span data-sub="${i}">${money(l.qty * (cents(l.price) || 0))}</span>`, `<button class="icon-btn" data-del="${i}" title="移除">×</button>`] };
    }), '尚未加入商品。請在上方輸入貨號、SKU 或品名。');
  $$('input[data-i]', box).forEach(inp => {
    inp.oninput = () => { const l = s.lines[inp.dataset.i]; l[inp.dataset.f] = inp.dataset.f === 'qty' ? (inp.value === '' ? '' : +inp.value) : inp.value; saleTotals(); };
    inp.onchange = renderSaleLines;
  });
  $$('[data-del]', box).forEach(b => b.onclick = () => { s.lines.splice(b.dataset.del, 1); renderSaleLines(); });
  saleTotals();
}

function saleTotals() {
  const s = drafts.sale;
  if (!s || !$('#s-total')) return;
  s.lines.forEach((l, i) => { const el = $(`[data-sub="${i}"]`); if (el) el.textContent = money(l.qty * (cents(l.price) || 0)); });
  const sub = s.lines.reduce((a, l) => a + (+l.qty || 0) * (cents(l.price) || 0), 0);
  const pcs = s.lines.reduce((a, l) => a + (+l.qty || 0), 0);
  const net = sub - (cents(s.discount) || 0), tax = taxOf(net, s.tax_type);
  $('#s-count').textContent = `${s.lines.length} 項・${pcs} 件・未稅 ${money(net)}・營業稅 ${money(tax)}（${s.tax_type}）`;
  $('#s-total').textContent = money(net + tax);
}

/** 與後端相同：應稅 5%，四捨五入到元 */
function taxOf(net, type) {
  return type === '應稅' ? Math.sign(net) * Math.round(Math.abs(net) * 5 / 10000) * 100 : 0;
}

async function submitSale() {
  const s = drafts.sale;
  if (!s.lines.length) return toast('請先加入商品', 'error');
  const bad = s.lines.find(l => !Number.isInteger(l.qty) || l.qty === 0);
  if (bad) return toast(`${bad.variant.sku} 的數量必須是不為 0 的整數`, 'error');
  const badPrice = s.lines.find(l => isNaN(cents(l.price)) || cents(l.price) < 0);
  if (badPrice) return toast(`${badPrice.variant.sku} 的單價不正確`, 'error');
  const discount = cents(s.discount) || 0;
  if (discount < 0) return toast('折扣不可為負數', 'error');
  const net = s.lines.reduce((a, l) => a + l.qty * cents(l.price), 0) - discount;
  const tax = taxOf(net, s.tax_type), total = net + tax;
  const pcs = s.lines.reduce((a, l) => a + l.qty, 0);
  const returns = s.lines.filter(l => l.qty < 0).length;
  if (!await confirmBox('確認結帳', `<p>${s.customer_name ? `${esc(s.customer_name)}・` : ''}${esc(s.payment)}・${s.lines.length} 項・${pcs} 件
      ${returns ? `<br><span class="red">含 ${returns} 項退貨</span>` : ''}${discount ? `<br>折扣 ${money(discount)}` : ''}
      <br>未稅 ${money(net)}・營業稅 ${money(tax)}（${esc(s.tax_type)}）</p>
      <p class="big-total">${money(total)}</p>`, '確認結帳')) return;
  const no = await call('create_doc', { input: { type: 'SO', date: s.date, payment: s.payment, tax_type: s.tax_type, discount, note: s.note,
    customer_name: s.customer_name, customer_phone: s.customer_phone, customer_address: s.customer_address,
    lines: s.lines.map(l => ({ variant_id: l.variant.id, qty: l.qty, unit_price: cents(l.price) })) } });
  toast(`已建立銷貨單 ${no}，實收 ${money(total)}`);
  drafts.sale = null;
  route();
}

// ---- 進貨 ----
PAGES.purchase = async main => {
  const p = drafts.purchase ??= { date: today(), supplier_id: '', note: '', photos: [], lines: [] };
  const suppliers = await call('suppliers');
  main.innerHTML = head('進貨', '步驟：選廠商 → 加入商品與數量 → 存檔（照片可附可不附）', '<button class="btn small" id="p-clear">清空重來</button>') + `
    <section class="card"><h2>1. 廠商與日期</h2><div class="toolbar">
      <label>廠商 *<select id="p-supplier"><option value="">請選擇廠商</option>
        ${suppliers.map(s => `<option value="${s.id}" ${s.id === +p.supplier_id ? 'selected' : ''}>${esc(s.code)} ${esc(s.name)}</option>`).join('')}</select></label>
      <button class="btn small" id="p-newsup">＋ 新增廠商</button>
      <label>進貨日期<input type="date" id="p-date" value="${p.date}" max="${today()}"></label>
      <label class="grow">備註<input id="p-note" value="${esc(p.note)}" style="width:100%"></label>
    </div></section>
    <section class="card"><h2>2. 照片<small>選填，例如廠商貨單或商品照</small></h2>
      <div class="photos" id="p-photos"></div>
      <label class="btn">選擇照片<input type="file" accept="image/*" multiple hidden id="p-file"></label></section>
    <section class="card"><h2>3. 商品明細</h2>
      <div class="row">${scanBox('p-scan', '輸入貨號、品名或廠商貨號找商品')}<button class="btn primary" id="p-newprod">＋ 新商品建檔</button></div>
      <div id="p-lines" style="margin-top:14px"></div></section>
    <div class="checkout"><div class="total"><span id="p-count"></span><b id="p-total"></b></div>
      <button class="btn primary big" id="p-submit">存檔進貨</button></div>`;
  $('#p-supplier').onchange = e => p.supplier_id = e.target.value;
  $('#p-date').oninput = e => p.date = e.target.value;
  $('#p-note').oninput = e => p.note = e.target.value;
  $('#p-clear').onclick = async () => {
    if ((p.lines.length || p.photos.length) && !await confirmBox('清空進貨單', '目前的照片與明細會全部清除，確定嗎？', '清空', true)) return;
    drafts.purchase = null; route();
  };
  $('#p-newsup').onclick = async () => { const id = await supplierDialog(); if (id && id !== 'deleted') { p.supplier_id = id; route(); } };
  $('#p-file').onchange = async e => {
    for (const f of e.target.files) p.photos.push(await uploadImage(f, 'purchases'));
    e.target.value = '';
    renderPhotos();
  };
  const addLines = lines => {
    for (const l of lines || []) {
      const ex = p.lines.find(x => x.variant.id === l.variant.id && x.price === l.price);
      if (ex) ex.qty += l.qty; else p.lines.push(l);
    }
    renderPurchaseLines();
  };
  const openProduct = async prod => addLines(await pickMatrix(prod, 'purchase'));
  bindScan('p-scan', { onVariant: async v => openProduct(await call('product', { id: v.product_id })), onProduct: openProduct });
  $('#p-newprod').onclick = async () => {
    const prod = await quickProduct({ supplier_id: +p.supplier_id || null });
    if (prod) openProduct(prod);
  };
  $('#p-submit').onclick = submitPurchase;
  renderPhotos();
  renderPurchaseLines();
};

function renderPhotos() {
  const p = drafts.purchase, box = $('#p-photos');
  box.innerHTML = p.photos.length ? p.photos.map((ph, i) => `<div class="ph"><img src="${MEDIA}${esc(ph)}" data-photo="${esc(ph)}" alt="">
    <button data-rm="${i}" title="移除">×</button></div>`).join('') : '<span class="muted">尚未附照片</span>';
  $$('[data-rm]', box).forEach(b => b.onclick = () => { p.photos.splice(b.dataset.rm, 1); renderPhotos(); });
}

function renderPurchaseLines() {
  const p = drafts.purchase, box = $('#p-lines');
  box.innerHTML = table(['SKU', '品名', '規格', ['數量', 'num'], ['進貨單價', 'num'], ['小計', 'num'], ''],
    p.lines.map((l, i) => [`<b>${esc(l.variant.sku)}</b>`, esc(l.variant.name), esc(`${l.variant.color}/${l.variant.size}`),
      `<input type="number" min="1" step="1" data-i="${i}" data-f="qty" value="${l.qty}" aria-label="數量">`,
      `<input type="number" min="0" step="1" data-i="${i}" data-f="price" value="${esc(l.price)}" aria-label="單價">`,
      money(l.qty * (cents(l.price) || 0)), `<button class="icon-btn" data-del="${i}" title="移除">×</button>`]),
    '尚未加入商品。搜尋既有商品，或按「＋ 新商品建檔」。');
  $$('input[data-i]', box).forEach(inp => inp.onchange = () => {
    const l = p.lines[inp.dataset.i];
    l[inp.dataset.f] = inp.dataset.f === 'qty' ? +inp.value : inp.value;
    renderPurchaseLines();
  });
  $$('[data-del]', box).forEach(b => b.onclick = () => { p.lines.splice(b.dataset.del, 1); renderPurchaseLines(); });
  const total = p.lines.reduce((a, l) => a + l.qty * (cents(l.price) || 0), 0);
  $('#p-count').textContent = `${p.lines.length} 項・${p.lines.reduce((a, l) => a + l.qty, 0)} 件`;
  $('#p-total').textContent = money(total);
}

async function submitPurchase() {
  const p = drafts.purchase;
  const problems = [
    !p.supplier_id && '請選擇廠商',
    !p.lines.length && '請加入商品明細',
    p.lines.some(l => !Number.isInteger(l.qty) || l.qty <= 0) && '進貨數量必須是大於 0 的整數',
    p.lines.some(l => isNaN(cents(l.price)) || cents(l.price) < 0) && '請填寫正確的進貨單價',
  ].filter(Boolean);
  if (problems.length) return toast(problems.join('；'), 'error');
  const total = p.lines.reduce((a, l) => a + l.qty * cents(l.price), 0);
  const sup = $('#p-supplier').selectedOptions[0].textContent;
  if (!await confirmBox('確認進貨', `<p>${esc(sup)}・${esc(p.date)}<br>${p.lines.length} 項・${p.lines.reduce((a, l) => a + l.qty, 0)} 件・照片 ${p.photos.length} 張</p>
      <p class="big-total">${money(total)}</p><p class="muted">存檔後不可修改，打錯只能作廢重開。</p>`, '確認存檔')) return;
  const no = await call('create_doc', { input: { type: 'PO', date: p.date, supplier_id: +p.supplier_id, note: p.note, photos: p.photos,
    lines: p.lines.map(l => ({ variant_id: l.variant.id, qty: l.qty, unit_price: cents(l.price) })) } });
  toast(`已建立進貨單 ${no}，庫存已增加`);
  drafts.purchase = null;
  route();
}

// ---- 庫存調整 ----
PAGES.adjust = async main => {
  const a = drafts.adjust ??= { date: today(), kind: META.kinds[0], variant: null, qty: '', counted: '', reason: '', note: '', lines: [] };
  const opening = () => a.kind === '期初庫存';
  main.innerHTML = head('庫存調整', '盤點差異、瑕疵報廢、退回廠商、開始使用時的期初庫存。每次調整都要填寫原因。') + `
    <section class="card"><h2>1. 選擇商品</h2>${scanBox('a-scan', '輸入貨號、SKU 或品名')}<div id="a-sel" style="margin-top:14px"></div></section>
    <section class="card"><h2>2. 調整內容</h2><div class="toolbar">
      <label>類型<select id="a-kind">${options(META.kinds, a.kind)}</select></label>
      <span id="a-qtybox"></span>
      <label>日期<input type="date" id="a-date" value="${a.date}" max="${today()}"></label>
      <label class="grow">原因 *<input id="a-reason" value="${esc(a.reason)}" placeholder="例如：月底盤點、車線脫落" style="width:100%"></label>
      <label class="grow">備註<input id="a-note" value="${esc(a.note)}" style="width:100%"></label>
    </div><p id="a-preview" class="hint"></p></section>
    <div class="checkout"><div class="total"><span>調整後庫存</span><b id="a-after">-</b></div><button class="btn primary big" id="a-submit">存檔調整</button></div>`;
  const select = v => { a.variant = v; render(); };
  const addOpening = async p => {
    for (const l of await pickMatrix(p, 'opening') || []) {
      const ex = a.lines.find(x => x.variant.id === l.variant.id);
      if (ex) Object.assign(ex, { qty: ex.qty + l.qty, price: l.price }); else a.lines.push(l);
    }
    render();
  };
  bindScan('a-scan', {
    onVariant: async v => opening() ? addOpening(await call('product', { id: v.product_id })) : select(v),
    onProduct: async p => { if (opening()) return addOpening(p); const v = await pickMatrix(p, 'pick'); if (v) select(v); },
  });
  const delta = () => {
    if (!a.variant) return NaN;
    if (a.kind === '盤點調整') return a.counted === '' ? NaN : +a.counted - a.variant.qty_on_hand;
    if (a.kind === '瑕疵報廢' || a.kind === '退回廠商') return a.qty === '' ? NaN : -Math.abs(+a.qty);
    return a.qty === '' ? NaN : +a.qty;
  };
  const preview = () => {
    const d = delta();
    $('#a-after').textContent = a.variant && !isNaN(d) ? `${a.variant.qty_on_hand} → ${a.variant.qty_on_hand + d}` : '-';
    $('#a-preview').textContent = a.variant && !isNaN(d) ? `本次調整 ${d > 0 ? '+' : ''}${d} 件` : '';
  };
  const render = () => {
    if (opening()) return renderOpening();
    const v = a.variant;
    $('#a-sel').innerHTML = v ? `<div class="row">${img(v.image)}<div><b>${esc(v.sku)}</b> ${esc(v.name)}<br>
      <span class="muted">${esc(v.color)}/${esc(v.size)}・目前庫存 <b>${v.qty_on_hand}</b> 件</span></div>
      <button class="btn small" id="a-unsel">重新選擇</button></div>` : '<div class="empty">尚未選擇商品</div>';
    $('#a-unsel')?.addEventListener('click', () => select(null));
    $('#a-qtybox').innerHTML = a.kind === '盤點調整'
      ? `<label>實際盤點數量<input type="number" id="a-counted" min="0" step="1" value="${esc(a.counted)}"></label>`
      : a.kind === '其他'
        ? `<label>增減數量（減少填負數）<input type="number" id="a-qty" step="1" value="${esc(a.qty)}"></label>`
        : `<label>減少件數<input type="number" id="a-qty" min="1" step="1" value="${esc(a.qty)}"></label>`;
    $('#a-counted')?.addEventListener('input', e => { a.counted = e.target.value; preview(); });
    $('#a-qty')?.addEventListener('input', e => { a.qty = e.target.value; preview(); });
    preview();
  };
  // 期初庫存：可一次輸入多個 SKU 的數量與單位成本（成本會納入平均成本）
  const renderOpening = () => {
    $('#a-qtybox').innerHTML = '';
    $('#a-sel').innerHTML = table(['SKU', '品名', '規格', ['目前庫存', 'num'], ['數量', 'num'], ['單位成本', 'num'], ['小計', 'num'], ''],
      a.lines.map((l, i) => [`<b>${esc(l.variant.sku)}</b>`, esc(l.variant.name), esc(`${l.variant.color}/${l.variant.size}`), l.variant.qty_on_hand,
        `<input type="number" min="1" step="1" data-i="${i}" data-f="qty" value="${l.qty}" aria-label="數量">`,
        `<input type="number" min="0" step="1" data-i="${i}" data-f="price" value="${esc(l.price)}" aria-label="單位成本">`,
        money(l.qty * (cents(l.price) || 0)), `<button class="icon-btn" data-del="${i}" title="移除">×</button>`]),
      '期初庫存：搜尋商品後，在顏色 × 尺寸表格填數量與單位成本，可一次加入多個商品。');
    $$('#a-sel input[data-i]').forEach(inp => inp.onchange = () => {
      const l = a.lines[inp.dataset.i];
      l[inp.dataset.f] = inp.dataset.f === 'qty' ? +inp.value : inp.value;
      renderOpening();
    });
    $$('#a-sel [data-del]').forEach(b => b.onclick = () => { a.lines.splice(b.dataset.del, 1); renderOpening(); });
    const pcs = a.lines.reduce((s, l) => s + l.qty, 0), cost = a.lines.reduce((s, l) => s + l.qty * (cents(l.price) || 0), 0);
    $('#a-after').textContent = `${pcs} 件`;
    $('#a-preview').textContent = a.lines.length ? `共 ${a.lines.length} 項・${pcs} 件・成本合計 ${money(cost)}` : '';
  };
  $('#a-kind').onchange = e => { a.kind = e.target.value; render(); };
  $('#a-date').oninput = e => a.date = e.target.value;
  $('#a-reason').oninput = e => a.reason = e.target.value;
  $('#a-note').oninput = e => a.note = e.target.value;
  $('#a-submit').onclick = async () => {
    if (opening()) {
      if (!a.lines.length) return toast('請先加入商品', 'error');
      if (a.lines.some(l => !Number.isInteger(l.qty) || l.qty <= 0)) return toast('數量必須是大於 0 的整數', 'error');
      if (a.lines.some(l => isNaN(cents(l.price)) || cents(l.price) < 0)) return toast('請填寫正確的單位成本', 'error');
      if (!a.reason.trim()) return toast('請填寫原因，例如「系統開始使用」', 'error');
      const cost = a.lines.reduce((s, l) => s + l.qty * cents(l.price), 0);
      if (!await confirmBox('確認期初庫存', `<p>${a.lines.length} 項・${a.lines.reduce((s, l) => s + l.qty, 0)} 件</p>
          <p class="big-total">成本 ${money(cost)}</p><p class="muted">存檔後不可修改，打錯只能作廢重開。</p>`, '確認存檔')) return;
      const no = await call('create_doc', { input: { type: 'AJ', date: a.date, kind: a.kind, reason: a.reason, note: a.note,
        lines: a.lines.map(l => ({ variant_id: l.variant.id, qty: l.qty, unit_price: cents(l.price) })) } });
      toast(`已建立期初庫存 ${no}`);
      drafts.adjust = null;
      return route();
    }
    const d = delta();
    if (!a.variant) return toast('請先選擇商品', 'error');
    if (isNaN(d) || !Number.isInteger(d)) return toast('請填寫整數數量', 'error');
    if (d === 0) return toast('盤點數量與目前庫存相同，不需要調整', 'error');
    if (!a.reason.trim()) return toast('請填寫調整原因', 'error');
    if (!await confirmBox('確認調整', `<p><b>${esc(a.variant.sku)}</b> ${esc(a.variant.name)}<br>${esc(a.kind)}：${esc(a.reason)}</p>
        <p class="big-total">${a.variant.qty_on_hand} → ${a.variant.qty_on_hand + d}</p>`, '確認存檔')) return;
    const no = await call('create_doc', { input: { type: 'AJ', date: a.date, kind: a.kind, reason: a.reason, note: a.note,
      lines: [{ variant_id: a.variant.id, qty: d }] } });
    toast(`已建立調整單 ${no}`);
    drafts.adjust = null;
    route();
  };
  render();
};

// ---- 查庫存 ----
PAGES.lookup = async (main, arg) => {
  main.innerHTML = head('查庫存', '輸入本店貨號、SKU 或品名') + `<section class="card">${scanBox('l-scan', '例如 4B7A695A9、素T')}</section><div id="l-out"></div>`;
  const show = p => {
    $('#l-out').innerHTML = `<section class="card"><div class="product-card">${img(p.image, 'preview')}<div class="info">
      <h2>${esc(p.code)} ${esc(p.name)}</h2>
      <p class="kv"><span>類別 <b>${esc(p.category)}</b></span><span>廠商 <b>${esc(p.supplier || '-')}</b></span>
        <span>廠商貨號 <b>${esc(p.supplier_code || '-')}</b></span><span>建議售價 <b>${money(p.list_price)}</b></span>
        <span>總庫存 <b>${p.stock}</b> 件</span></p>
      ${matrixTable(p, stockCell)}
      <p class="hint">紅字：低於安全庫存；灰字：沒有庫存</p>
      <div class="row"><a class="btn small" href="#/product/${p.id}">編輯商品</a><a class="btn small" href="#/docs/${encodeURIComponent(p.code)}">相關單據</a></div>
      </div></div></section>`;
  };
  bindScan('l-scan', { onProduct: show });
  if (arg) { $('#l-scan').value = arg; $('[data-scan="l-scan"]').click(); }
};

// ---- 單據查詢 ----
PAGES.docs = async (main, arg) => {
  const f = drafts.docFilter ??= { type: '', start: monthStart(), end: today(), q: '' };
  if (arg) Object.assign(f, { q: arg, start: '', end: '' });
  main.innerHTML = head('單據查詢', '點選單據可查看明細或作廢') + `
    <section class="card"><div class="toolbar">
      <div class="tabs">${[['', '全部'], ['SO', '銷貨'], ['PO', '進貨'], ['AJ', '調整']].map(([k, l]) => `<button data-t="${k}" class="${f.type === k ? 'on' : ''}">${l}</button>`).join('')}</div>
      <label>起<input type="date" id="d-start" value="${f.start}"></label><label>迄<input type="date" id="d-end" value="${f.end}"></label>
      <label class="grow">關鍵字<input type="search" id="d-q" value="${esc(f.q)}" placeholder="單號、SKU、貨號、品名" style="width:100%"></label>
    </div></section><section class="card" id="d-out"></section>`;
  const load = async () => {
    const list = await call('docs', { filter: f });
    $('#d-out').innerHTML = `<p class="muted">共 ${list.length} 張${list.length >= 500 ? '（只顯示最新 500 張，請縮小範圍）' : ''}</p>` + table(DOC_HEADS, docRows(list), '這個條件下沒有單據');
  };
  $$('[data-t]').forEach(b => b.onclick = () => { f.type = b.dataset.t; $$('[data-t]').forEach(x => x.classList.toggle('on', x === b)); load(); });
  $('#d-start').onchange = e => { f.start = e.target.value; load(); };
  $('#d-end').onchange = e => { f.end = e.target.value; load(); };
  $('#d-q').oninput = debounce(e => { f.q = e.target.value; load(); });
  await load();
};

// ---- 歷史銷貨（匯入的舊 Excel 銷貨單，只做紀錄，不影響庫存）----
PAGES.history = async main => {
  const f = drafts.historyFilter ??= { start: '', end: '', q: '' };
  main.innerHTML = head('歷史銷貨', '匯入舊的 Excel 銷貨單，只做紀錄與營業額統計，不影響庫存',
      '<label class="btn primary">匯入 Excel 銷貨單<input type="file" id="h-file" accept=".xlsx,.xls,.xlsm" multiple hidden></label>') + `
    <section class="card"><div class="toolbar">
      <label>起<input type="date" id="h-start" value="${f.start}"></label><label>迄<input type="date" id="h-end" value="${f.end}"></label>
      <label class="grow">關鍵字<input type="search" id="h-q" value="${esc(f.q)}" placeholder="單號、客戶、品名、檔名" style="width:100%"></label>
    </div><p class="hint">一個檔案可以有多個工作表，每個工作表一張銷貨單。系統依「品名、數量、單價、小計」表頭找欄位，日期可用民國年（例 115/06/27）。同一個單號不會重複匯入。</p></section>
    <section class="card" id="h-out"></section>`;
  const load = async () => {
    const list = await call('histories', f);
    const sum = list.reduce((s, h) => s + h.total, 0);
    $('#h-out').innerHTML = `<p class="muted">共 ${list.length} 張・總計 ${money(sum)}</p>` +
      table(['日期', '單號', '客戶', ['品項', 'num'], ['件數', 'num'], ['銷貨總額', 'num'], ['營業稅', 'num'], ['總計', 'num'], '來源檔案'],
        list.map(h => ({ attrs: `data-hist="${h.id}" data-href=""`, cells: [esc(h.date), `<b>${esc(h.no || '-')}</b>`, esc(h.customer_name || '-'),
          h.lines, h.qty, money(h.subtotal), money(h.tax), money(h.total), `<span class="muted">${esc(h.source)}</span>`] })),
        '還沒有匯入任何歷史銷貨單');
    $$('[data-hist]').forEach(tr => tr.onclick = () => showHistory(+tr.dataset.hist));
  };
  $('#h-start').onchange = e => { f.start = e.target.value; load(); };
  $('#h-end').onchange = e => { f.end = e.target.value; load(); };
  $('#h-q').oninput = debounce(e => { f.q = e.target.value; load(); });
  $('#h-file').onchange = async e => {
    const done = [], skipped = [];
    for (const file of e.target.files) {
      try {
        const r = await invoke('import_history', new Uint8Array(await file.arrayBuffer()), { headers: { 'x-name': encodeURIComponent(file.name) } });
        done.push(...r.imported); skipped.push(...r.skipped);
      } catch (err) { skipped.push(String(err)); }
    }
    e.target.value = '';
    const d = modal(`<h2>匯入結果</h2><div class="dlg-body">
      <p><b>成功匯入 ${done.length} 張</b></p>${done.length ? `<ul>${done.map(x => `<li>${esc(x)}</li>`).join('')}</ul>` : ''}
      ${skipped.length ? `<p class="err">略過 ${skipped.length} 項</p><ul>${skipped.map(x => `<li>${esc(x)}</li>`).join('')}</ul>` : ''}
      </div><div class="actions"><button class="btn primary">確定</button></div>`);
    $('.btn', d).onclick = () => d.close();
    load();
  };
  await load();
};

async function showHistory(id) {
  const h = await call('history_doc', { id });
  const d = modal(`<h2>歷史銷貨 ${esc(h.no || '')}</h2><div class="dlg-body">
    <p class="kv"><span>日期：<b>${esc(h.date)}</b></span>${h.customer_name ? `<span>客戶：<b>${esc(h.customer_name)}</b></span>` : ''}
      ${h.customer_phone ? `<span>電話：<b>${esc(h.customer_phone)}</b></span>` : ''}${h.customer_address ? `<span>地址：<b>${esc(h.customer_address)}</b></span>` : ''}
      <span>來源：<b>${esc(h.source)}</b></span><span>匯入時間：<b>${esc(h.imported_at)}</b></span></p>
    ${table(['貨號', '品名', '規格', ['數量', 'num'], ['單價', 'num'], ['小計', 'num'], '備註'],
      h.lines.map(l => [esc(l.code), esc(l.name), esc(l.spec), l.qty, money(l.unit_price), money(l.subtotal), esc(l.note)]), '這張單沒有品項明細')}
    <p style="text-align:right">銷貨總額 ${money(h.subtotal)}　營業稅 ${money(h.tax)}　<b class="big-total">${money(h.total)}</b></p></div>
    <div class="actions"><button class="btn danger" id="hd-del" style="margin-right:auto">刪除這筆匯入</button><button class="btn" id="hd-close">關閉</button></div>`, true);
  $('#hd-close', d).onclick = () => d.close();
  $('#hd-del', d).onclick = async () => {
    if (!await confirmBox('刪除歷史銷貨', '只會刪除這筆匯入的紀錄（不影響庫存），刪除會寫進操作紀錄。確定嗎？', '刪除', true)) return;
    await call('delete_history', { id });
    toast('已刪除');
    d.close();
    route();
  };
}

function debounce(fn, ms = 300) { let t; return (...a) => { clearTimeout(t); t = setTimeout(() => fn(...a), ms); }; }

// ---- 商品 ----
PAGES.products = async main => {
  drafts.productQ ??= '';
  main.innerHTML = head('商品', '點選商品可修改資料、安全庫存', '<a class="btn primary" href="#/product/new">＋ 新增商品</a>') + `
    <section class="card"><input type="search" id="pr-q" value="${esc(drafts.productQ)}" placeholder="搜尋貨號、品名、廠商貨號、SKU" style="width:100%"></section>
    <section class="card" id="pr-out"></section>`;
  const load = async () => {
    const list = await call('products', { q: drafts.productQ });
    $('#pr-out').innerHTML = table(['', '本店貨號', '品名', '類別', '廠商', '廠商貨號', ['建議售價', 'num'], ['SKU 數', 'num'], ['總庫存', 'num']],
      list.map(p => ({ attrs: `data-href="#/product/${p.id}"`, cells: [img(p.image), `<b>${esc(p.code)}</b>`, esc(p.name), esc(p.category),
        esc(p.supplier || '-'), esc(p.supplier_code || '-'), money(p.list_price), p.skus, p.stock] })),
      drafts.productQ ? '找不到符合的商品' : '還沒有商品，按右上角「＋ 新增商品」開始建檔');
  };
  $('#pr-q').oninput = debounce(e => { drafts.productQ = e.target.value; load(); });
  $('#pr-q').focus();
  await load();
};

PAGES.product = async (main, arg) => {
  const p = arg === 'new' ? { name: '', category: '', supplier_id: null, supplier_code: '', list_price: 0, image: '', note: '', variants: [] }
    : await call('product', { id: +arg });
  main.innerHTML = head(p.id ? `${esc(p.code)} ${esc(p.name)}` : '新增商品', p.id ? `建檔日期 ${esc(p.code_date)}・本店貨號建立後不可修改` : '存檔後自動產生本店貨號',
      `<a class="btn" href="#/products">返回列表</a>${p.id ? `<a class="btn" href="#/lookup/${encodeURIComponent(p.code)}">查庫存</a>` : ''}`) + `
    <section class="card"><h2>商品資料</h2><form id="pf">${await productForm(p)}</form>
      <div class="row" style="margin-top:16px"><button class="btn primary" id="pf-save">${p.id ? '儲存變更' : '建立商品'}</button>
      ${p.id ? '<button class="btn danger" id="pf-del" style="margin-left:auto">刪除商品</button>' : ''}</div></section>
    ${p.id ? `<section class="card"><h2>SKU<small>安全庫存修改後自動儲存</small></h2>${table(
      ['SKU', '顏色', '尺寸', ['安全庫存', 'num'], ['庫存', 'num'], ['平均成本', 'num'], ''],
      p.variants.map(v => [`<b>${esc(v.sku)}</b>`, esc(v.color), esc(v.size),
        `<input type="number" min="0" step="1" data-vs="${v.id}" value="${v.safety_stock}">`,
        v.qty_on_hand, money(v.avg_cost),
        v.used ? '<span class="muted" title="已有單據，不可刪除">使用中</span>' : `<button class="icon-btn" data-vdel="${v.id}" title="刪除 SKU">×</button>`]),
      '還沒有 SKU，請在上方勾選顏色與尺寸後儲存')}</section>` : ''}`;
  const form = $('#pf');
  bindProductForm(form);
  $('#pf-save').onclick = async () => {
    const input = readProductForm(form, p.id);
    if (!input.name.trim()) return toast('請填寫品名', 'error');
    const id = await call('save_product', { input });
    toast(p.id ? '已儲存商品' : '已建立商品');
    if (p.id) route(); else location.hash = `#/product/${id}`;
  };
  $('#pf-del')?.addEventListener('click', async () => {
    if (!await confirmBox('刪除商品', `確定刪除「${esc(p.name)}」及其所有 SKU？<br><span class="muted">已有進銷貨紀錄的商品無法刪除。</span>`, '刪除', true)) return;
    await call('delete_item', { what: 'product', id: p.id });
    toast('已刪除商品');
    location.hash = '#/products';
  });
  $$('[data-vs]').forEach(inp => inp.onchange = async () => {
    const safety = +inp.value;
    if (!Number.isInteger(safety) || safety < 0) return toast('安全庫存必須是 0 以上的整數', 'error');
    try { await call('update_variant', { id: +inp.dataset.vs, safety }); toast('已儲存'); inp.classList.remove('invalid'); }
    catch { inp.classList.add('invalid'); }
  });
  $$('[data-vdel]').forEach(b => b.onclick = async () => {
    if (!await confirmBox('刪除 SKU', '確定刪除這個 SKU？', '刪除', true)) return;
    await call('delete_item', { what: 'variant', id: +b.dataset.vdel });
    toast('已刪除 SKU');
    route();
  });
};

// ---- 廠商 ----
PAGES.suppliers = async main => {
  const list = await call('suppliers');
  main.innerHTML = head('廠商', '點選廠商可修改', '<button class="btn primary" id="su-add">＋ 新增廠商</button>') +
    `<section class="card">${table(['代號', '名稱', '聯絡人', '電話', '地址', '備註'],
      list.map(s => ({ attrs: `data-sup="${s.id}" data-href=""`, cells: [`<b>${esc(s.code)}</b>`, esc(s.name), esc(s.contact), esc(s.phone), esc(s.address), esc(s.note)] })),
      '還沒有廠商，按右上角「＋ 新增廠商」')}</section>`;
  $('#su-add').onclick = async () => { if (await supplierDialog()) route(); };
  $$('[data-sup]').forEach(tr => tr.onclick = async () => { if (await supplierDialog(list.find(s => s.id === +tr.dataset.sup))) route(); });
};

// ---- 顏色與尺寸 ----
PAGES.specs = async main => {
  const [colors, sizes] = await Promise.all([call('colors'), call('sizes')]);
  const del = (what, x) => x.used ? '<span class="muted">使用中</span>' : `<button class="icon-btn" data-del-${what}="${x.id}" title="刪除">×</button>`;
  main.innerHTML = head('顏色與尺寸', '代碼會組成 SKU（例如 4B7A695A9-BK-M），建立後不可修改；名稱與排序可隨時調整。') + `<div class="grid2">
    <section class="card"><h2>顏色</h2>${table(['代碼', '名稱', ''], colors.map(c => [`<b>${esc(c.code)}</b>`,
      `<input data-cname="${c.id}" value="${esc(c.name)}">`, del('color', c)]))}
      <div class="row" style="margin-top:12px"><input id="c-code" placeholder="代碼，例 OR" style="width:120px"><input id="c-name" placeholder="名稱，例 橘">
      <button class="btn" id="c-add">新增顏色</button></div></section>
    <section class="card"><h2>尺寸</h2>${table(['代碼', ['排序（小的在前）', 'num'], ''], sizes.map(s => [`<b>${esc(s.code)}</b>`,
      `<input type="number" step="1" data-ssort="${s.id}" value="${s.sort}">`, del('size', s)]))}
      <div class="row" style="margin-top:12px"><input id="z-code" placeholder="代碼，例 4XL" style="width:120px"><input type="number" id="z-sort" placeholder="排序" step="1">
      <button class="btn" id="z-add">新增尺寸</button></div></section></div>`;
  $$('[data-cname]').forEach(i => i.onchange = async () => { await call('save_color', { input: { id: +i.dataset.cname, name: i.value } }); toast('已儲存'); });
  $$('[data-ssort]').forEach(i => i.onchange = async () => { await call('save_size', { input: { id: +i.dataset.ssort, sort: +i.value || 0 } }); toast('已儲存'); });
  $$('[data-del-color]').forEach(b => b.onclick = async () => { await call('delete_item', { what: 'color', id: +b.dataset.delColor }); toast('已刪除'); route(); });
  $$('[data-del-size]').forEach(b => b.onclick = async () => { await call('delete_item', { what: 'size', id: +b.dataset.delSize }); toast('已刪除'); route(); });
  $('#c-add').onclick = async () => { await call('save_color', { input: { code: $('#c-code').value, name: $('#c-name').value } }); toast('已新增顏色'); route(); };
  $('#z-add').onclick = async () => { await call('save_size', { input: { code: $('#z-code').value, sort: +$('#z-sort').value || 0 } }); toast('已新增尺寸'); route(); };
};

// ---- 報表 ----
const REPORTS = [['inventory', '即時庫存表'], ['purchases', '進貨報表'], ['sales', '銷貨報表'], ['quarterly', '季度收益'], ['monthly', '月營業額'], ['slow', '滯銷品'], ['top', '熱銷排行'], ['history', '歷史銷貨']];

PAGES.reports = async (main, arg) => {
  const name = REPORTS.some(r => r[0] === arg) ? arg : 'inventory';
  const prm = drafts.reportParams ??= { start: monthStart(), end: today(), days: 60 };
  const ranged = ['purchases', 'sales', 'top', 'history'].includes(name);
  main.innerHTML = head('報表', '可匯出 Excel；「更新三大報表」產生進貨、銷貨、收益三個工作表',
      '<button class="btn" id="r-three">更新三大報表</button><button class="btn" data-open="reports">開啟報表資料夾</button>') + `
    <section class="card"><div class="tabs">${REPORTS.map(([k, l]) => `<a href="#/reports/${k}" class="${k === name ? 'on' : ''}">${l}</a>`).join('')}</div>
      <div class="toolbar" style="margin-top:14px">
        ${ranged ? `<label>起<input type="date" id="r-start" value="${prm.start}"></label><label>迄<input type="date" id="r-end" value="${prm.end}"></label>` : ''}
        ${name === 'slow' ? `<label>幾天內沒賣出<input type="number" id="r-days" min="1" step="1" value="${prm.days}"></label>` : ''}
        <button class="btn primary" id="r-export">匯出 Excel</button><span class="muted" id="r-count"></span>
      </div></section><section class="card" id="r-out"></section>`;
  const load = async () => {
    const rep = await call('report', { name, params: prm });
    $('#r-count').textContent = `${rep.title}・共 ${rep.rows.length} 筆`;
    const heads = rep.headers.map(([h, k]) => (k === 'money' || k === 'int') ? [h, 'num'] : h);
    $('#r-out').innerHTML = table(heads, rep.rows.map(r => r.map((v, i) => {
      const k = rep.headers[i][1];
      return k === 'img' ? img(v) : k === 'money' ? money(v) : esc(v ?? '');
    })));
  };
  const bind = (id, key, num) => { const el = $(id); if (el) el.onchange = () => { prm[key] = num ? +el.value : el.value; load(); }; };
  bind('#r-start', 'start'); bind('#r-end', 'end'); bind('#r-days', 'days', true);
  $('#r-export').onclick = async () => toast(`已匯出：${await call('export_report', { name, params: prm })}`);
  $('#r-three').onclick = async () => toast(`已更新：${await call('export_three')}`);
  await load();
};

// ---- 操作紀錄 ----
PAGES.audit = async main => {
  main.innerHTML = head('操作紀錄', '所有新增、修改、作廢、匯出都會記錄在這裡，紀錄無法修改或刪除。') +
    `<section class="card"><input type="search" id="au-q" placeholder="搜尋動作、單號、貨號、內容或日期（例 2026-10）" style="width:100%"></section>
    <section class="card" id="au-out"></section>`;
  const load = async q => {
    const list = await call('audit', { q });
    $('#au-out').innerHTML = `<p class="muted">${list.length >= 1000 ? '只顯示最新 1000 筆，請用搜尋縮小範圍' : `共 ${list.length} 筆`}</p>` +
      table(['時間', '動作', '對象', '內容'], list.map(a => [esc(a.at), `<b>${esc(a.action)}</b>`, esc(a.target), esc(a.detail)]));
  };
  $('#au-q').oninput = debounce(e => load(e.target.value));
  await load('');
};

PAGES.help = async main => { main.innerHTML = $('#help-page').innerHTML; };

// ---------- 路由 ----------
async function route() {
  const [name, ...rest] = location.hash.replace(/^#\/?/, '').split('/');
  const page = PAGES[name] ? name : 'home';
  const arg = decodeURIComponent(rest.join('/'));
  const navKey = page === 'product' ? 'products' : page;
  $$('#nav a').forEach(a => a.classList.toggle('on', a.dataset.page === navKey));
  const main = $('#main');
  $$('dialog').forEach(d => d.close());
  try {
    await PAGES[page](main, arg);
  } catch (e) {
    main.innerHTML = `<div class="error-box">畫面載入失敗：${esc(e)}</div>`;
  }
  main.scrollTop = 0;
}

document.addEventListener('click', e => {
  const t = e.target.closest('[data-doc], [data-href], [data-open], [data-photo], a[href^="#help-"]');
  if (!t || (e.target.closest('input, button') && !t.matches('button'))) return;
  if (t.matches('a[href^="#help-"]')) { e.preventDefault(); $(t.getAttribute('href'))?.scrollIntoView({ behavior: 'smooth' }); }
  else if (t.dataset.doc) showDoc(+t.dataset.doc);
  else if (t.dataset.open) call('open_folder', { which: t.dataset.open });
  else if (t.dataset.photo) showPhoto(t.dataset.photo);
  else if (t.dataset.href) location.hash = t.dataset.href;
});

// 主題：記在本機，下次開啟沿用
const THEMES = [['walnut', '胡桃木'], ['mountain', '山海'], ['bamboo', '竹林'], ['night', '星夜'], ['sakura', '櫻花'], ['ink', '水墨']];
function applyTheme(key) {
  const t = THEMES.find(x => x[0] === key) || THEMES[0];
  document.documentElement.dataset.theme = t[0];
  $('#theme-btn').textContent = `主題：${t[1]}`;
  try { localStorage.setItem('theme', t[0]); } catch { /* 無法儲存時只影響本次 */ }
}
$('#theme-btn').onclick = () => {
  const i = THEMES.findIndex(x => x[0] === document.documentElement.dataset.theme);
  applyTheme(THEMES[(i + 1) % THEMES.length][0]);
};
try { applyTheme(localStorage.getItem('theme')); } catch { applyTheme(); }

window.addEventListener('hashchange', route);
call('meta').then(m => { META = m; route(); });
