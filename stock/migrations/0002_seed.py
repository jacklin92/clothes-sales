from django.db import migrations

COLORS = [('BK', '黑'), ('WH', '白'), ('GY', '灰'), ('NV', '深藍'), ('BL', '藍'), ('BE', '米'), ('KH', '卡其'),
          ('BR', '咖啡'), ('RD', '紅'), ('PK', '粉'), ('GN', '綠'), ('YL', '黃')]
SIZES = ['XS', 'S', 'M', 'L', 'XL', '2XL', '3XL', 'F']


def seed(apps, schema_editor):
    Color, Size = apps.get_model('stock', 'Color'), apps.get_model('stock', 'Size')
    for code, name in COLORS:
        Color.objects.get_or_create(code=code, defaults={'name': name})
    for i, code in enumerate(SIZES):
        Size.objects.get_or_create(code=code, defaults={'sort': i * 10})


class Migration(migrations.Migration):
    dependencies = [('stock', '0001_initial')]
    operations = [migrations.RunPython(seed, migrations.RunPython.noop)]
