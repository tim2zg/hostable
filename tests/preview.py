"""Start a temporary local UI preview with the Proxmox fixture. Enter stops it.
The fixed preview token is deliberately disposable; this never opens live metadata.
"""
import hashlib,sqlite3
from test_api import API
try:
    API.setUpClass()
    connection=sqlite3.connect(API.temp.name+'/metadata.db')
    token='hst_local_preview_for_ui_checks'
    connection.execute("UPDATE users SET api_token=? WHERE username='admin'",(hashlib.sha256(token.encode()).hexdigest(),)); connection.commit(); connection.close()
    print('Preview URL: '+API.base,flush=True)
    print('Disposable preview token: '+token,flush=True)
    input('Press Enter to stop the preview.\n')
finally: API.doClassCleanups()
