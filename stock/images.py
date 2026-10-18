from io import BytesIO
from pathlib import Path

from django.core.files.base import ContentFile
from PIL import Image, ImageOps

MAX_SIDE = 1600


def compress_upload(field):
    """新上傳的圖片：依 EXIF 轉正、縮到長邊 1600px、存成 JPEG。已存檔的圖片不動。"""
    if not field or getattr(field, '_committed', True):
        return
    img = ImageOps.exif_transpose(Image.open(field))
    img = img.convert('RGB')
    img.thumbnail((MAX_SIDE, MAX_SIDE))
    buf = BytesIO()
    img.save(buf, 'JPEG', quality=85, optimize=True)
    field.save(Path(field.name).stem + '.jpg', ContentFile(buf.getvalue()), save=False)


def thumbnail_bytes(path, size=80):
    img = Image.open(path)
    img = img.convert('RGB')
    img.thumbnail((size, size))
    buf = BytesIO()
    img.save(buf, 'JPEG', quality=80)
    buf.seek(0)
    return buf
