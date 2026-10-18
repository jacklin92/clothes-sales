from django.conf import settings
from django.contrib import admin
from django.contrib.staticfiles.views import serve as serve_static
from django.urls import path, re_path
from django.views.generic import RedirectView
from django.views.static import serve as serve_media

from stock.views import lookup, report, update_reports

urlpatterns = [
    path('', RedirectView.as_view(url='/admin/')),
    path('admin/', admin.site.urls),
    path('reports/<str:name>/', report, name='report'),
    path('update-reports/', update_reports, name='update_reports'),
    path('lookup/', lookup, name='lookup'),
    # 本機單機使用，靜態檔與圖片直接由 Django 提供（exe 版 DEBUG 關閉時也要能用）
    re_path(r'^static/(?P<path>.*)$', serve_static, {'insecure': True}),
    re_path(r'^media/(?P<path>.*)$', serve_media, {'document_root': settings.MEDIA_ROOT}),
]
