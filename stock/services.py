"""過帳、作廢與成本重算（移動加權平均）。"""
from decimal import Decimal, ROUND_HALF_UP

from django.db import transaction

from .models import Adjustment, Document, Purchase, Sale, StockMove, Variant

CENT = Decimal('0.01')


def recalc_variant(variant: Variant):
    """依日期重播此 SKU 所有異動，算出每筆的成本、結存，以及目前庫存與平均成本。

    同一天先處理入庫再處理出庫，補登過去日期的單據也能得到正確成本。
    """
    moves = sorted(variant.moves.select_related('purchase_line'), key=lambda m: (m.date, m.qty < 0, m.id))
    qty, value, avg = 0, Decimal(0), Decimal(0)
    for m in moves:
        if m.kind == StockMove.PURCHASE:
            cost = m.purchase_line.unit_price
        else:
            cost = avg  # 銷貨、退貨、異動都以當下平均成本計
        qty += m.qty
        value += m.qty * cost
        if qty <= 0:
            value = Decimal(0)
        else:
            avg = (value / qty).quantize(CENT, ROUND_HALF_UP)
        m.unit_cost, m.balance_qty, m.balance_value = cost, qty, value.quantize(CENT, ROUND_HALF_UP)
    StockMove.objects.bulk_update(moves, ['unit_cost', 'balance_qty', 'balance_value'])
    variant.qty_on_hand, variant.avg_cost = qty, avg
    variant.save(update_fields=['qty_on_hand', 'avg_cost'])


def _move_specs(doc):
    if isinstance(doc, Purchase):
        return [dict(variant=l.variant, qty=l.qty, kind=StockMove.PURCHASE, purchase_line=l)
                for l in doc.lines.all()]
    if isinstance(doc, Sale):
        return [dict(variant=l.variant, qty=-l.qty, kind=StockMove.SALE, sale_line=l)
                for l in doc.lines.all()]
    if isinstance(doc, Adjustment):
        return [dict(variant=doc.variant, qty=doc.qty, kind=StockMove.ADJUST, adjustment=doc)]
    raise TypeError(doc)


def _existing_moves(doc):
    if isinstance(doc, Purchase):
        return StockMove.objects.filter(purchase_line__purchase=doc)
    if isinstance(doc, Sale):
        return StockMove.objects.filter(sale_line__sale=doc)
    return StockMove.objects.filter(adjustment=doc)


@transaction.atomic
def post(doc):
    """依單據內容重建庫存帳並重算受影響的 SKU。"""
    old = _existing_moves(doc)
    affected = {m.variant_id for m in old}
    old.delete()
    if doc.status == Document.POSTED:
        for spec in _move_specs(doc):
            StockMove.objects.create(date=doc.date, **spec)
            affected.add(spec['variant'].id)
    for v in Variant.objects.filter(id__in=affected):
        recalc_variant(v)


@transaction.atomic
def void(doc):
    doc.status = Document.VOID
    doc.save(update_fields=['status'])
    post(doc)
