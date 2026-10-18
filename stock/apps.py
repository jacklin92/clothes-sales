from django.apps import AppConfig
from django.db.models.signals import post_migrate

# 店員：可銷貨、查庫存（看不到成本），不能進貨
CLERK_PERMS = ['view_product', 'view_variant', 'view_color', 'view_size',
               'add_sale', 'view_sale', 'add_adjustment', 'view_adjustment']


def setup_groups(sender, **kwargs):
    from django.contrib.auth.models import Group, Permission

    perms = Permission.objects.filter(content_type__app_label='stock')
    Group.objects.get_or_create(name='店長')[0].permissions.set(perms)
    Group.objects.get_or_create(name='店員')[0].permissions.set(perms.filter(codename__in=CLERK_PERMS))


class StockConfig(AppConfig):
    default_auto_field = 'django.db.models.BigAutoField'
    name = 'stock'
    verbose_name = '庫存管理'

    def ready(self):
        post_migrate.connect(setup_groups, sender=self)
