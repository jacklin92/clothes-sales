"""本店貨號：{YYYYMMDD}{3 碼當日流水號} 視為十進位整數，轉成大寫 16 進位。

例：2026-10-17 第 1 款 → 20261017001 → 4B7A695A9
"""
import datetime


def encode_product_code(date: datetime.date, serial: int) -> str:
    if not 1 <= serial <= 999:
        raise ValueError('當日流水號需介於 1~999')
    return format(int(f'{date:%Y%m%d}{serial:03d}'), 'X')


def decode_product_code(code: str) -> tuple[datetime.date, int]:
    raw = str(int(code, 16))
    return datetime.datetime.strptime(raw[:8], '%Y%m%d').date(), int(raw[8:])
