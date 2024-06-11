# -*- coding: utf-8 -*-

__version__ = "1.0.0"

import sys
import loguru

def main():
    print("Example version %s." % __version__)
    print("Argument strings: %s" % sys.argv[1:])
    print("loguru version %s" % loguru.__version__)
