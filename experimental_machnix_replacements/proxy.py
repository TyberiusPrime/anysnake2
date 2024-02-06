"""
 This is an experimental pypi-proxying-server
 that filters out the links to new version of packages.

 It does this by intercepting the /simple/ requests,
 querying the json metadata from pypi, then filtering
 to only those urls that were released before threshold_date.


"""
import sys
import re
from http.server import BaseHTTPRequestHandler, HTTPServer
import urllib3
import pprint
import time
import dateutil.parser

hostName = "localhost"
serverPort = 8080

pypi = "https://pypi.org"
http = urllib3.PoolManager()

threshold_date = dateutil.parser.isoparse("2023-02-01T00:00:00+00:00")


def get_pypi_meta(module):
    url = pypi + "/pypi/" + module + "/json"
    print(url)
    response = http.request("GET", url)
    return response.json()


def extract_ok_links(meta_json, threshold_date):
    result = set()
    for release, info in meta_json["releases"].items():
        for entry in info:
            iso_time = entry["upload_time_iso_8601"]
            dt = dateutil.parser.isoparse(iso_time)
            if dt < threshold_date:
                result.add(entry["url"])
    return result


def filter_simple_page(simple_page, ok_links):
    def check_link(match):
        url = match.group(1)
        if "#" in url:
            url = url.split("#", 1)[0]
        if url in ok_links:
            return match.group(0)
        else:
            return ""

    return re.sub('<a href="([^"]+)"[^>]*.[^<]+</a>', check_link, simple_page)


class MyServer(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path.startswith("/simple/"):
            module = self.path.split("/", 2)[2]
            if module.endswith("/"):
                module = module[:-1]
            print("module is", module)
            try:
                meta = get_pypi_meta(module)
                ok_links = extract_ok_links(meta, threshold_date)
                url = pypi + self.path
                response = http.request("GET", url)
                simple_page = response.data.decode("utf-8")
                filtered = filter_simple_page(simple_page, ok_links)
                print(filtered)
                self.send_response(200)
                self.send_header("Content-type", "text/html")
                self.end_headers()
                self.wfile.write(filtered.encode("utf-8"))
            except Exception as e:
                self.send_response(500)
                self.send_header("Content-type", "text/html")
                self.end_headers()
                self.wfile.write(repr(e).encode("utf-8"))
                raise


if __name__ == "__main__":
    webServer = HTTPServer((hostName, serverPort), MyServer)
    print("Server started http://%s:%s" % (hostName, serverPort))

    try:
        webServer.serve_forever()
    except KeyboardInterrupt:
        pass

    webServer.server_close()
