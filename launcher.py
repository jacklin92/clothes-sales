"""exe 進入點：建立/升級資料庫 → 第一次執行時建立管理員 → 啟動本機伺服器並開啟瀏覽器。"""
import os
import socket
import sys
import threading
import webbrowser

os.environ.setdefault('DJANGO_SETTINGS_MODULE', 'config.settings')

HOST, PORT = '127.0.0.1', int(os.environ.get('SALES_PORT', 8000))
URL = f'http://{HOST}:{PORT}/'


def port_in_use():
    with socket.socket() as s:
        return s.connect_ex((HOST, PORT)) == 0


def main():
    if port_in_use():  # 已經開著了，直接開瀏覽器
        webbrowser.open(URL)
        return

    import django
    from django.core.management import call_command
    django.setup()

    print('檢查資料庫…')
    call_command('migrate', verbosity=0, interactive=False)

    from django.conf import settings
    from django.contrib.auth.models import User
    if not User.objects.filter(is_superuser=True).exists():
        print('\n第一次使用，請建立管理員帳號（店長）：')
        call_command('createsuperuser')

    from waitress import serve
    from config.wsgi import application

    print(f'\n資料位置：{settings.DATA_DIR}')
    print(f'系統已啟動：{URL}')
    print('使用期間請勿關閉此視窗；關閉視窗即結束系統。')
    if not os.environ.get('SALES_NO_BROWSER'):  # 自動測試時不開瀏覽器
        threading.Timer(1.5, webbrowser.open, [URL]).start()
    serve(application, host=HOST, port=PORT, threads=8, _quiet=True)


if __name__ == '__main__':
    try:
        main()
    except Exception:
        import traceback
        traceback.print_exc()
        input('\n發生錯誤，按 Enter 關閉…')
        sys.exit(1)
