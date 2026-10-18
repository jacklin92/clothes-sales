from decimal import Decimal

from django import template
from django.utils.html import format_html

register = template.Library()


@register.simple_tag
def cell(value, index, image_cols):
    if index in image_cols:
        if not value:
            return format_html('<td></td>')
        return format_html('<td><a href="{0}" target="_blank"><img src="{0}"></a></td>', value.url)
    if isinstance(value, (int, Decimal)):
        return format_html('<td class="num">{}</td>', f'{value:,}')
    return format_html('<td>{}</td>', '' if value is None else value)
