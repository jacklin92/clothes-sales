"""報表：每個報表回傳 Report，HTML 與 Excel 共用同一份資料。

欄位名稱前加 * 的是成本相關欄位，沒有 view_cost 權限的人看不到。
"""
import datetime
from dataclasses import dataclass, field
from decimal import Decimal

from django.db.models import Max, Min, Sum, F

from .models import Document, Product, PurchaseLine, Sale, SaleLine, StockMove, Variant

IMAGE = 'image'  # 欄位型別標記：該欄存放 ImageFieldFile


@dataclass
class Report:
    title: str
    headers: list
    rows: list = field(default_factory=list)
    needs_cost: bool = False  # 整份報表都需要成本權限

    def strip_cost(self):
        keep = [i for i, h in enumerate(self.headers) if not str(h).startswith('*')]
        self.headers = [self.headers[i] for i in keep]
        self.rows = [[r[i] for i in keep] for r in self.rows]

    def clean_headers(self):
        return [h[0] if isinstance(h, tuple) else str(h).lstrip('*') for h in self.headers]

    def image_cols(self):
        return {i for i, h in enumerate(self.headers) if isinstance(h, tuple) and h[1] == IMAGE}


def pct(x):
    return f'{x:.1%}' if x is not None else '-'


def _variant_qs():
    return Variant.objects.select_related('product', 'color', 'size')


def inventory(params):
    last_in = dict(StockMove.objects.filter(kind=StockMove.PURCHASE)
                   .values_list('variant').annotate(d=Max('date')))
    r = Report('即時庫存表', [('圖片', IMAGE), '本店貨號', 'SKU', '品名', '顏色', '尺寸', '目前庫存',
                              '*平均成本', '*庫存金額', '最後進貨日', '安全庫存', '警示'])
    for v in _variant_qs().order_by('product__code', 'color__code', 'size__sort'):
        warn = '低於安全庫存' if v.qty_on_hand < v.safety_stock else ''
        r.rows.append([v.product.image, v.product.code, v.sku, v.product.name, v.color.name, v.size.code,
                       v.qty_on_hand, v.avg_cost, v.avg_cost * v.qty_on_hand, last_in.get(v.id), v.safety_stock,
                       warn])
    return r


def purchases(params):
    r = Report(f'進貨報表 {params["start"]} ~ {params["end"]}',
               ['進貨日期', '單號', '廠商', '品名', '顏色/尺寸', '數量', '單價', '總成本', '廠商貨號', '本店貨號',
                ('照片', IMAGE)], needs_cost=True)
    lines = (PurchaseLine.objects
             .filter(purchase__status=Document.POSTED, purchase__date__range=(params['start'], params['end']))
             .select_related('purchase__supplier', 'variant__product', 'variant__color', 'variant__size')
             .prefetch_related('purchase__photos')
             .order_by('purchase__date', 'purchase__no', 'id'))
    for l in lines:
        p, v = l.purchase, l.variant
        photo = next(iter(p.photos.all()), None)
        r.rows.append([p.date, p.no, p.supplier.name, v.product.name, f'{v.color.name}/{v.size.code}',
                       l.qty, l.unit_price, l.total, v.product.supplier_code, v.product.code,
                       photo.image if photo else None])
    return r


def sales(params):
    r = Report(f'銷貨報表 {params["start"]} ~ {params["end"]}',
               ['銷貨日期', '單號', '通路', '本店貨號', '品名', '顏色/尺寸', '數量', '單價', '小計', '*成本',
                '*毛利'])
    lines = (SaleLine.objects
             .filter(sale__status=Document.POSTED, sale__date__range=(params['start'], params['end']))
             .select_related('sale', 'move', 'variant__product', 'variant__color', 'variant__size')
             .order_by('sale__date', 'sale__no', 'id'))
    for l in lines:
        s, v = l.sale, l.variant
        cost = l.qty * l.move.unit_cost
        r.rows.append([s.date, s.no, s.get_channel_display(), v.product.code, v.product.name,
                       f'{v.color.name}/{v.size.code}', l.qty, l.unit_price, l.subtotal, cost, l.subtotal - cost])
    return r


def inventory_value(as_of):
    """as_of 當天結束時的庫存總金額。"""
    last = {}
    for m in StockMove.objects.filter(date__lte=as_of):
        key = (m.date, m.qty < 0, m.id)
        if m.variant_id not in last or key > last[m.variant_id][0]:
            last[m.variant_id] = (key, m.balance_value)
    return sum((v for _, v in last.values()), Decimal(0))


def _quarter_start(d):
    return datetime.date(d.year, 3 * ((d.month - 1) // 3) + 1, 1)


def _next_quarter(d):
    return datetime.date(d.year + (d.month == 10), (d.month + 2) % 12 + 1, 1)


def quarterly(params):
    r = Report('季度收益', ['季度', '進貨成本', '銷貨總額', '銷貨成本', '毛利', '毛利率', '期初庫存',
                           '期末庫存', '週轉率'], needs_cost=True)
    first = StockMove.objects.aggregate(d=Min('date'))['d']
    if not first:
        return r
    q = _quarter_start(first)
    while q <= params['today']:
        end = _next_quarter(q) - datetime.timedelta(days=1)
        rng = (q, end)
        buy = (PurchaseLine.objects.filter(purchase__status=Document.POSTED, purchase__date__range=rng)
               .aggregate(s=Sum(F('qty') * F('unit_price')))['s'] or Decimal(0))
        gross = (SaleLine.objects.filter(sale__status=Document.POSTED, sale__date__range=rng)
                 .aggregate(s=Sum(F('qty') * F('unit_price')))['s'] or Decimal(0))
        discount = (Sale.objects.filter(status=Document.POSTED, date__range=rng)
                    .aggregate(s=Sum('discount'))['s'] or Decimal(0))
        revenue = gross - discount
        cogs = -(StockMove.objects.filter(kind=StockMove.SALE, date__range=rng)
                 .aggregate(s=Sum(F('qty') * F('unit_cost')))['s'] or Decimal(0))
        opening = inventory_value(q - datetime.timedelta(days=1))
        closing = inventory_value(end)
        avg_inv = (opening + closing) / 2
        r.rows.append([f'{q.year} Q{(q.month - 1) // 3 + 1}', buy, revenue, cogs, revenue - cogs,
                       pct((revenue - cogs) / revenue) if revenue else '-', opening, closing,
                       f'{cogs / avg_inv:.2f}' if avg_inv else '-'])
        q = _next_quarter(q)
    return r


def slow(params):
    days = params['days']
    cutoff = params['today'] - datetime.timedelta(days=days)
    last_sale = dict(StockMove.objects.filter(kind=StockMove.SALE)
                     .values_list('variant').annotate(d=Max('date')))
    first_in = dict(StockMove.objects.filter(kind=StockMove.PURCHASE)
                    .values_list('variant').annotate(d=Min('date')))
    r = Report(f'滯銷品（{days} 天內無銷售且有庫存）',
               [('圖片', IMAGE), '本店貨號', 'SKU', '品名', '顏色', '尺寸', '目前庫存', '最後銷貨日', '首次進貨日'])
    for v in _variant_qs().filter(qty_on_hand__gt=0):
        ls, fi = last_sale.get(v.id), first_in.get(v.id)
        if (ls or fi or params['today']) <= cutoff:
            r.rows.append([v.product.image, v.product.code, v.sku, v.product.name, v.color.name, v.size.code,
                           v.qty_on_hand, ls, fi])
    r.rows.sort(key=lambda row: row[7] or row[8] or datetime.date.min)
    return r


def top(params):
    r = Report(f'熱銷排行 {params["start"]} ~ {params["end"]}',
               [('圖片', IMAGE), '本店貨號', '品名', '銷售數量', '銷售額'])
    rows = (SaleLine.objects
            .filter(sale__status=Document.POSTED, sale__date__range=(params['start'], params['end']))
            .values('variant__product').annotate(q=Sum('qty'), amt=Sum(F('qty') * F('unit_price')))
            .order_by('-q')[:50])
    products = Product.objects.in_bulk([x['variant__product'] for x in rows])
    for x in rows:
        p = products[x['variant__product']]
        r.rows.append([p.image, p.code, p.name, x['q'], x['amt']])
    return r


def three_reports(today):
    """對應系統規劃.xlsx 的三個工作表（全部期間）。回傳 [(工作表名, Report)]。"""
    params = {'today': today, 'start': datetime.date(2000, 1, 1), 'end': today, 'days': 60}
    return [('進貨系統', purchases(params)), ('銷貨系統', sales(params)), ('收益計算', quarterly(params))]


def lookup(q):
    """依本店貨號 / SKU / 條碼 / 品名找商品。"""
    q = q.strip()
    if not q:
        return []
    exact = Product.objects.filter(code__iexact=q.split('-')[0])
    if exact:
        return list(exact)
    v = Variant.objects.filter(barcode=q).select_related('product').first()
    if v:
        return [v.product]
    return list(Product.objects.filter(name__icontains=q)[:20])


def stock_matrix(product):
    """顏色 × 尺寸的庫存表。"""
    variants = list(product.variants.select_related('color', 'size'))
    sizes = sorted({v.size for v in variants}, key=lambda s: (s.sort, s.code))
    colors = sorted({v.color for v in variants}, key=lambda c: c.code)
    cell = {(v.color_id, v.size_id): v for v in variants}
    rows = [(c, [cell.get((c.id, s.id)) for s in sizes]) for c in colors]
    return sizes, rows, sum(v.qty_on_hand for v in variants)


REPORTS = {
    'inventory': inventory,
    'purchases': purchases,
    'sales': sales,
    'quarterly': quarterly,
    'slow': slow,
    'top': top,
}
