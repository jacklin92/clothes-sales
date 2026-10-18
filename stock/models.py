from decimal import Decimal

from django.conf import settings
from django.core.validators import MinValueValidator
from django.db import models
from django.db.models import Max
from django.utils import timezone

from .codes import encode_product_code
from .images import compress_upload

MONEY = dict(max_digits=12, decimal_places=2)


def next_doc_no(model, prefix, date):
    """單號：前綴 + YYYYMMDD + 3 碼流水號，例 PO20261017001。"""
    head = f'{prefix}{date:%Y%m%d}'
    last = model.objects.filter(no__startswith=head).aggregate(m=Max('no'))['m']
    return f'{head}{int(last[-3:]) + 1 if last else 1:03d}'


class Supplier(models.Model):
    code = models.CharField('廠商代號', max_length=20, unique=True)
    name = models.CharField('廠商名稱', max_length=100)
    contact = models.CharField('聯絡人', max_length=50, blank=True)
    phone = models.CharField('電話', max_length=50, blank=True)
    address = models.CharField('地址', max_length=200, blank=True)
    note = models.TextField('備註', blank=True)

    class Meta:
        verbose_name = verbose_name_plural = '廠商'
        ordering = ['code']

    def __str__(self):
        return f'{self.code} {self.name}'


class Color(models.Model):
    code = models.CharField('顏色代碼', max_length=6, unique=True, help_text='用於 SKU，例：BK')
    name = models.CharField('顏色', max_length=20)

    class Meta:
        verbose_name = verbose_name_plural = '顏色'
        ordering = ['code']

    def __str__(self):
        return f'{self.name}({self.code})'


class Size(models.Model):
    code = models.CharField('尺寸代碼', max_length=6, unique=True)
    sort = models.PositiveSmallIntegerField('排序', default=0)

    class Meta:
        verbose_name = verbose_name_plural = '尺寸'
        ordering = ['sort', 'code']

    def __str__(self):
        return self.code


class Product(models.Model):
    CATEGORIES = [('top', '上衣'), ('bottom', '褲/裙'), ('dress', '洋裝'), ('outer', '外套'),
                  ('acc', '配件'), ('other', '其他')]

    code = models.CharField('本店貨號', max_length=12, unique=True, editable=False)
    code_date = models.DateField('建檔日期', editable=False)
    code_serial = models.PositiveSmallIntegerField('當日流水號', editable=False)
    name = models.CharField('品名', max_length=100)
    category = models.CharField('類別', max_length=10, choices=CATEGORIES, default='top')
    supplier = models.ForeignKey(Supplier, models.PROTECT, verbose_name='預設廠商', null=True, blank=True)
    supplier_code = models.CharField('廠商貨號', max_length=50, blank=True)
    list_price = models.DecimalField('建議售價', **MONEY, default=0)
    image = models.ImageField('商品主圖', upload_to='products/%Y/%m/', blank=True)
    note = models.TextField('備註', blank=True)

    class Meta:
        verbose_name = verbose_name_plural = '商品'
        ordering = ['-code_date', '-code_serial']
        constraints = [models.UniqueConstraint(fields=['code_date', 'code_serial'], name='uniq_product_serial')]

    def __str__(self):
        return f'{self.code} {self.name}'

    def save(self, *args, **kwargs):
        if not self.code:
            self.code_date = timezone.localdate()
            last = Product.objects.filter(code_date=self.code_date).aggregate(m=Max('code_serial'))['m'] or 0
            self.code_serial = last + 1
            self.code = encode_product_code(self.code_date, self.code_serial)
        compress_upload(self.image)
        super().save(*args, **kwargs)


class Variant(models.Model):
    product = models.ForeignKey(Product, models.CASCADE, related_name='variants', verbose_name='商品')
    color = models.ForeignKey(Color, models.PROTECT, verbose_name='顏色')
    size = models.ForeignKey(Size, models.PROTECT, verbose_name='尺寸')
    sku = models.CharField('SKU', max_length=30, unique=True, editable=False)
    barcode = models.CharField('條碼', max_length=50, blank=True, db_index=True)
    safety_stock = models.PositiveIntegerField('安全庫存', default=0)
    # 以下兩欄由 services.recalc_variant 依異動紀錄重算，勿手動修改
    qty_on_hand = models.IntegerField('目前庫存', default=0, editable=False)
    avg_cost = models.DecimalField('平均成本', **MONEY, default=0, editable=False)

    class Meta:
        verbose_name = verbose_name_plural = '商品規格(SKU)'
        ordering = ['product', 'color__code', 'size__sort']
        constraints = [models.UniqueConstraint(fields=['product', 'color', 'size'], name='uniq_variant')]
        permissions = [('view_cost', '可查看成本與毛利')]

    def __str__(self):
        return f'{self.sku} {self.product.name} {self.color.name}/{self.size.code}'

    def save(self, *args, **kwargs):
        self.sku = f'{self.product.code}-{self.color.code}-{self.size.code}'
        super().save(*args, **kwargs)


class Document(models.Model):
    """進貨單、銷貨單、庫存異動的共同欄位。單據不可刪除，只能作廢。"""
    POSTED, VOID = 'posted', 'void'
    STATUSES = [(POSTED, '已過帳'), (VOID, '作廢')]

    no = models.CharField('單號', max_length=20, unique=True, editable=False)
    date = models.DateField('日期', default=timezone.localdate)
    status = models.CharField('狀態', max_length=10, choices=STATUSES, default=POSTED, editable=False)
    note = models.TextField('備註', blank=True)
    created_by = models.ForeignKey(settings.AUTH_USER_MODEL, models.PROTECT, verbose_name='建檔人',
                                   null=True, editable=False)
    created_at = models.DateTimeField('建檔時間', auto_now_add=True)

    PREFIX = ''

    class Meta:
        abstract = True
        ordering = ['-date', '-no']

    def __str__(self):
        return self.no

    def save(self, *args, **kwargs):
        if not self.no:
            self.no = next_doc_no(type(self), self.PREFIX, self.date)
        super().save(*args, **kwargs)


class Purchase(Document):
    PREFIX = 'PO'
    supplier = models.ForeignKey(Supplier, models.PROTECT, verbose_name='廠商')

    class Meta(Document.Meta):
        verbose_name = verbose_name_plural = '進貨單'

    @property
    def total(self):
        return sum((l.total for l in self.lines.all()), Decimal(0))


class PurchasePhoto(models.Model):
    purchase = models.ForeignKey(Purchase, models.CASCADE, related_name='photos')
    image = models.ImageField('照片', upload_to='purchases/%Y/%m/')

    class Meta:
        verbose_name = verbose_name_plural = '進貨照片'

    def save(self, *args, **kwargs):
        compress_upload(self.image)
        super().save(*args, **kwargs)


class PurchaseLine(models.Model):
    purchase = models.ForeignKey(Purchase, models.CASCADE, related_name='lines')
    variant = models.ForeignKey(Variant, models.PROTECT, verbose_name='SKU')
    qty = models.PositiveIntegerField('數量', validators=[MinValueValidator(1)])
    unit_price = models.DecimalField('單價', **MONEY)

    class Meta:
        verbose_name = verbose_name_plural = '進貨明細'

    @property
    def total(self):
        return self.qty * self.unit_price


class Sale(Document):
    PREFIX = 'SO'
    CHANNELS = [('store', '門市'), ('online', '網路'), ('wholesale', '批發')]
    PAYMENTS = [('cash', '現金'), ('card', '信用卡'), ('transfer', '轉帳'), ('mobile', '行動支付')]

    channel = models.CharField('通路', max_length=10, choices=CHANNELS, default='store')
    payment = models.CharField('付款方式', max_length=10, choices=PAYMENTS, default='cash')
    discount = models.DecimalField('整單折扣金額', **MONEY, default=0)

    class Meta(Document.Meta):
        verbose_name = verbose_name_plural = '銷貨單'

    @property
    def total(self):
        return sum((l.subtotal for l in self.lines.all()), Decimal(0)) - self.discount


class SaleLine(models.Model):
    sale = models.ForeignKey(Sale, models.CASCADE, related_name='lines')
    variant = models.ForeignKey(Variant, models.PROTECT, verbose_name='SKU')
    qty = models.IntegerField('數量', help_text='客人退貨請填負數')
    unit_price = models.DecimalField('單價', **MONEY)

    class Meta:
        verbose_name = verbose_name_plural = '銷貨明細'

    @property
    def subtotal(self):
        return self.qty * self.unit_price


class Adjustment(Document):
    PREFIX = 'AJ'
    KINDS = [('count', '盤點調整'), ('damage', '瑕疵報廢'), ('return_supplier', '退回廠商'), ('other', '其他')]

    kind = models.CharField('類型', max_length=20, choices=KINDS)
    variant = models.ForeignKey(Variant, models.PROTECT, verbose_name='SKU')
    qty = models.IntegerField('數量', help_text='增加填正數，減少填負數')
    reason = models.CharField('原因', max_length=200)

    class Meta(Document.Meta):
        verbose_name = verbose_name_plural = '庫存異動'


class StockMove(models.Model):
    """庫存帳：每一筆進出都在這裡，庫存與成本都由此重算。"""
    PURCHASE, SALE, ADJUST = 'purchase', 'sale', 'adjust'
    KINDS = [(PURCHASE, '進貨'), (SALE, '銷貨'), (ADJUST, '異動')]

    variant = models.ForeignKey(Variant, models.PROTECT, related_name='moves')
    date = models.DateField(db_index=True)
    kind = models.CharField(max_length=10, choices=KINDS)
    qty = models.IntegerField()
    unit_cost = models.DecimalField(**MONEY, default=0)
    balance_qty = models.IntegerField(default=0)
    balance_value = models.DecimalField(max_digits=14, decimal_places=2, default=0)
    purchase_line = models.OneToOneField(PurchaseLine, models.CASCADE, null=True, related_name='move')
    sale_line = models.OneToOneField(SaleLine, models.CASCADE, null=True, related_name='move')
    adjustment = models.OneToOneField(Adjustment, models.CASCADE, null=True, related_name='move')

    class Meta:
        ordering = ['date', 'id']
