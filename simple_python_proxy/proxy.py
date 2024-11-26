#!/usr/bin/env python3
import os
import socketserver
import http.server
import tempfile
import urllib.request
import urllib.error
import urllib.parse
import re


# Do not follow redirect just return whatever the other server returns
class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


opener = urllib.request.build_opener(NoRedirect)
urllib.request.install_opener(opener)


class simpleProxy(http.server.SimpleHTTPRequestHandler):
    server_version = "simpleProxy"

    def do_GET(self):
        res = self.do_request(method="GET")
        if isinstance(res, http.client.HTTPResponse) and res.status == 200:
            length = 64 * 1024
            dest_dir = os.path.dirname(res.cache_file)
            os.makedirs(dest_dir, exist_ok=True)
            # We write to a temp file and rename at the end, so we don't save a partial file in case of disconnect
            fd, path = tempfile.mkstemp(
                prefix=os.path.basename(res.cache_file) + ".",
                dir=dest_dir,
                suffix=".temp~",
            )
            f = open(path, mode="wb")
            while True:
                buf = res.read(length)
                if not buf:
                    break
                self.wfile.write(buf)
                f.write(buf)
            f.close()
            os.rename(path, res.cache_file)
        else:
            self.copyfile(res, self.wfile)

    def do_HEAD(self):
        self.do_request(method="HEAD")

    def transform_path(self, url):
        # Could be overwritten by implementation
        parts = urllib.parse.urlsplit(url)

        # just 2 level domain, no port
        path = ".".join(parts.netloc.split(".")[-2:]).split(":")[0]
        # remove session/XXXXXX from path
        path = path + re.sub(r"/session/[^/]*", "", parts.path)

        return path

    def do_request(self, method="GET"):
        # remove the separator so it only leaves http....
        url = self.path.lstrip("/?")

        path = ".cache/"
        path = path + self.transform_path(url)

        if os.path.isfile(path):
            self.path = path
            # Hack to log the request as local file
            self.requestline = path
            return self.send_head()
        else:
            # get the headers and delete the proxy Host
            headers = self.headers
            del headers["Host"]
            # Make the request to the server
            req = urllib.request.Request(url, headers=headers, method=method)
            try:
                res = urllib.request.urlopen(req)
            except urllib.error.HTTPError as e:
                res = e
            # Log the request and return the status and headers
            self.log_request(res.status)
            self.send_response_only(res.status)
            # So we can write it later
            res.cache_file = path
            for key, val in res.getheaders():
                self.send_header(key, val)
            self.end_headers()

            # return the result for further processing
            return res


if __name__ == "__main__":
    PORT = 8088

    httpd = socketserver.ForkingTCPServer(("", PORT), simpleProxy)
    print("Now serving at", str(PORT))
    httpd.serve_forever()
