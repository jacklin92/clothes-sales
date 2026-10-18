from collections import Counter

from django import forms
from django.contrib import admin, messages
from django.core.exceptions import ValidationError
from django.db.models import Sum
from django.utils.html import format_html

from . import services
from .models import (Adjustment, Color, Document, Product, Purchase, PurchaseLine, PurchasePhoto, Sale,
                     SaleLine, Size, Supplier, Variant)

admin.site.site_header = admin.site.site_title = '服飾庫存系統'
admin.site.index_title = '功能選單'


def thumb(image, size=60):
    if not image:
        return '-'
    return format_html('<a href="{0}" target="_blank"><img src="{0}" style="max-height:{1}px;max-width:{1}px"></a>',
                       image.url, size)


def can_view_cost(request):
    return request.user.has_perm('stock.view_cost')


@admin.register(Supplier)
class SupplierAdmin(admin.ModelAdmin):
    list_display = ['code', 'name', 'contact', 'phone']
    search_fields = ['code', 'name']


@admin.register(Color)
class ColorAdmin(admin.ModelAdmin):
    list_display = ['code', 'name']


@admin.register(Size)
class SizeAdmin(admin.ModelAdmin):
    list_display = ['code', 'sort']
    list_editable = ['sort']


# ---------- 商品 ----------

class ProductForm(forms.ModelForm):
    add_colors = forms.ModelMultipleChoiceField(Color.objects, required=False, label='批次產生 SKU：顏色',
                                                widget=forms.CheckboxSelectMultiple)
    add_sizes = forms.ModelMultipleChoiceField(Size.objects, required=False, label='批次產生 SKU：尺寸',
                                               widget=forms.CheckboxSelectMultiple,
                                               help_text='勾選的顏色 × 尺寸會自動建立 SKU（已存在的略過）')

    class Meta:
        model = Product
        fields = '__all__'


class VariantInline(admin.TabularInline):
    model = Variant
    extra = 0
    fields = ['sku', 'color', 'size', 'barcode', 'safety_stock', 'qty_on_hand', 'avg_cost']
    can_delete = False

    def get_readonly_fields(self, request, obj=None):
        return ['sku', 'qty_on_hand', 'avg_cost']

    def get_fields(self, request, obj=None):
        fields = list(super().get_fields(request, obj))
        return fields if can_view_cost(request) else [f for f in fields if f != 'avg_cost']


@admin.register(Product)
class ProductAdmin(admin.ModelAdmin):
    form = ProductForm
    inlines = [VariantInline]
    list_display = ['image_thumb', 'code', 'name', 'category', 'supplier', 'supplier_code', 'list_price',
                    'stock_total']
    list_display_links = ['image_thumb', 'code', 'name']
    list_filter = ['category', 'supplier']
    search_fields = ['code', 'name', 'supplier_code']
    readonly_fields = ['code', 'code_date', 'image_preview']
    fieldsets = [
        (None, {'fields': ['code', 'code_date', 'name', 'category', 'supplier', 'supplier_code', 'list_price',
                           'image', 'image_preview', 'note']}),
        ('SKU', {'fields': ['add_colors', 'add_sizes']}),
    ]

    @admin.display(description='圖片')
    def image_thumb(self, obj):
        return thumb(obj.image)

    @admin.display(description='預覽')
    def image_preview(self, obj):
        return thumb(obj.image, 200)

    @admin.display(description='總庫存')
    def stock_total(self, obj):
        return obj.variants.aggregate(s=Sum('qty_on_hand'))['s'] or 0

    def save_related(self, request, form, formsets, change):
        super().save_related(request, form, formsets, change)
        for color in form.cleaned_data['add_colors']:
            for size in form.cleaned_data['add_sizes']:
                Variant.objects.get_or_create(product=form.instance, color=color, size=size)


@admin.register(Variant)
class VariantAdmin(admin.ModelAdmin):
    list_display = ['sku', 'product_name', 'color', 'size', 'qty_on_hand', 'safety_stock', 'avg_cost']
    list_filter = ['product__category', 'color', 'size']
    search_fields = ['sku', 'barcode', 'product__name', 'product__code']
    list_select_related = ['product', 'color', 'size']
    readonly_fields = ['product', 'color', 'size', 'sku', 'qty_on_hand', 'avg_cost']

    @admin.display(description='品名')
    def product_name(self, obj):
        return obj.product.name

    def get_list_display(self, request):
        ld = super().get_list_display(request)
        return ld if can_view_cost(request) else [f for f in ld if f != 'avg_cost']

    def get_fields(self, request, obj=None):
        fields = super().get_fields(request, obj)
        return fields if can_view_cost(request) else [f for f in fields if f != 'avg_cost']

    def has_add_permission(self, request):
        return False  # SKU 由商品頁建立

    def has_delete_permission(self, request, obj=None):
        return False


# ---------- 單據共用：建立後唯讀、不可刪、只能作廢 ----------

class LockedInline(admin.TabularInline):
    extra = 0
    can_delete = False

    def has_change_permission(self, request, obj=None):
        return obj is None

    def has_add_permission(self, request, obj=None):
        return obj is None


@admin.action(description='作廢選取的單據')
def void_documents(modeladmin, request, queryset):
    n = 0
    for doc in queryset.filter(status=Document.POSTED):
        services.void(doc)
        n += 1
    modeladmin.message_user(request, f'已作廢 {n} 張單據，庫存與成本已重算。', messages.SUCCESS)


class DocumentAdmin(admin.ModelAdmin):
    actions = [void_documents]
    date_hierarchy = 'date'
    list_filter = ['status']
    search_fields = ['no']

    def get_readonly_fields(self, request, obj=None):
        if obj:  # 已過帳的單據不能修改，打錯請作廢重開
            return [f.name for f in self.model._meta.fields] + list(self.readonly_fields)
        return self.readonly_fields

    def has_delete_permission(self, request, obj=None):
        return False

    def save_model(self, request, obj, form, change):
        if not change:
            obj.created_by = request.user
        super().save_model(request, obj, form, change)

    def save_related(self, request, form, formsets, change):
        super().save_related(request, form, formsets, change)
        if not change:
            services.post(form.instance)


# ---------- 進貨 ----------

class PhotoFormSet(forms.BaseInlineFormSet):
    def clean(self):
        super().clean()
        if not any(f.cleaned_data.get('image') for f in self.forms if f.cleaned_data):
            raise ValidationError('進貨單至少要附一張照片。')


class PurchasePhotoInline(LockedInline):
    model = PurchasePhoto
    formset = PhotoFormSet
    min_num = 1
    fields = ['image', 'preview']
    readonly_fields = ['preview']

    @admin.display(description='預覽')
    def preview(self, obj):
        return thumb(obj.image, 120)


class PurchaseLineInline(LockedInline):
    model = PurchaseLine
    min_num = 1
    autocomplete_fields = ['variant']
    fields = ['variant', 'product_name', 'supplier_code', 'product_code', 'qty', 'unit_price', 'total']
    readonly_fields = ['product_name', 'supplier_code', 'product_code', 'total']

    @admin.display(description='品名')
    def product_name(self, obj):
        return obj.variant.product.name if obj.pk else ''

    @admin.display(description='廠商貨號')
    def supplier_code(self, obj):
        return obj.variant.product.supplier_code if obj.pk else ''

    @admin.display(description='本店貨號')
    def product_code(self, obj):
        return obj.variant.product.code if obj.pk else ''

    @admin.display(description='總成本')
    def total(self, obj):
        return obj.total if obj.pk else ''


@admin.register(Purchase)
class PurchaseAdmin(DocumentAdmin):
    inlines = [PurchasePhotoInline, PurchaseLineInline]
    list_display = ['photo_thumb', 'no', 'date', 'supplier', 'doc_total', 'status', 'created_by']
    list_filter = ['status', 'supplier']
    fields = ['no', 'date', 'supplier', 'note', 'status', 'created_by']
    readonly_fields = ['no', 'status', 'created_by']

    @admin.display(description='照片')
    def photo_thumb(self, obj):
        photo = obj.photos.first()
        return thumb(photo.image if photo else None)

    @admin.display(description='總成本')
    def doc_total(self, obj):
        return obj.total


# ---------- 銷貨 ----------

class SaleLineForm(forms.ModelForm):
    class Meta:
        model = SaleLine
        fields = ['variant', 'qty', 'unit_price']

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.fields['unit_price'].required = False
        self.fields['unit_price'].help_text = '留空＝建議售價'

    def clean(self):
        data = super().clean()
        if data.get('variant') and data.get('unit_price') is None:
            data['unit_price'] = data['variant'].product.list_price
        if data.get('qty') == 0:
            self.add_error('qty', '數量不可為 0')
        return data


class SaleLineFormSet(forms.BaseInlineFormSet):
    def clean(self):
        super().clean()
        need = Counter()
        for f in self.forms:
            if f.cleaned_data and not f.cleaned_data.get('DELETE') and f.cleaned_data.get('variant'):
                need[f.cleaned_data['variant']] += f.cleaned_data['qty']
        short = [f'{v.sku}（庫存 {v.qty_on_hand}，要出 {q}）' for v, q in need.items() if q > v.qty_on_hand]
        if short:
            raise ValidationError('庫存不足：' + '、'.join(short))


class SaleLineInline(LockedInline):
    model = SaleLine
    form = SaleLineForm
    formset = SaleLineFormSet
    min_num = 1
    autocomplete_fields = ['variant']
    fields = ['variant', 'qty', 'unit_price', 'subtotal']
    readonly_fields = ['subtotal']

    @admin.display(description='小計')
    def subtotal(self, obj):
        return obj.subtotal if obj.pk else ''


@admin.register(Sale)
class SaleAdmin(DocumentAdmin):
    inlines = [SaleLineInline]
    list_display = ['no', 'date', 'channel', 'payment', 'doc_total', 'status', 'created_by']
    list_filter = ['status', 'channel', 'payment']
    fields = ['no', 'date', 'channel', 'payment', 'discount', 'note', 'status', 'created_by']
    readonly_fields = ['no', 'status', 'created_by']

    @admin.display(description='銷貨淨額')
    def doc_total(self, obj):
        return obj.total


# ---------- 庫存異動 ----------

class AdjustmentForm(forms.ModelForm):
    class Meta:
        model = Adjustment
        fields = '__all__'

    def clean(self):
        data = super().clean()
        v, qty = data.get('variant'), data.get('qty')
        if qty == 0:
            self.add_error('qty', '數量不可為 0')
        elif v and qty and v.qty_on_hand + qty < 0:
            self.add_error('qty', f'庫存不足：目前 {v.qty_on_hand}')
        return data


@admin.register(Adjustment)
class AdjustmentAdmin(DocumentAdmin):
    form = AdjustmentForm
    autocomplete_fields = ['variant']
    list_display = ['no', 'date', 'kind', 'variant', 'qty', 'reason', 'status', 'created_by']
    list_filter = ['status', 'kind']
    fields = ['no', 'date', 'kind', 'variant', 'qty', 'reason', 'note', 'status', 'created_by']
    readonly_fields = ['no', 'status', 'created_by']
