"""Exercise surface routing through the actual HTTP contract mock."""
import json
from pathlib import Path
import threading
import unittest
from urllib.error import HTTPError
from urllib.request import Request, urlopen

from tools.mock_server import MockServer

ROOT=Path(__file__).resolve().parents[1]

class MockRoutingTests(unittest.TestCase):
    def test_connector_matches_site_manager_and_proxies_network(self):
        connector='/v1/connector/consoles/fixture-console/proxy/network/integration/v1/sites'
        protect='/v1/connector/consoles/fixture-console/proxy/protect/integration/v1/meta/info'
        for surface,path,expected in [('site-manager',connector,connector),('network',connector,'/v1/sites'),('protect',protect,'/v1/meta/info')]:
            with self.subTest(surface=surface):
                spec=json.loads((ROOT/'schemas'/f'{surface}.json').read_text())
                server=MockServer(('127.0.0.1',0),spec)
                thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
                try:
                    url=f'http://127.0.0.1:{server.server_port}{path}'
                    with self.assertRaises(HTTPError) as missing_key:urlopen(url,timeout=5)
                    self.assertEqual(missing_key.exception.code,401)
                    missing_key.exception.close()
                    with urlopen(Request(url,headers={'X-API-Key':'mock-only-key'}),timeout=5) as response:
                        self.assertEqual(response.status,200)
                        response.read()
                    self.assertEqual(server.requests,[('GET',expected),('GET',expected)])
                finally:server.shutdown();server.server_close();thread.join()

if __name__=='__main__':unittest.main()
