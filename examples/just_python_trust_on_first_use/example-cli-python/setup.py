# -*- coding: utf-8 -*-


"""setup.py: setuptools control."""


import re
from setuptools import setup


version = re.search(
    '^__version__\s*=\s*"(.*)"',
    open('hello/hello.py').read(),
    re.M
    ).group(1)


with open("README.rst", "rb") as f:
    long_descr = f.read().decode("utf-8")

setup(
    name = "example-cli-python",
    packages = ["hello"],
    entry_points = {
        "console_scripts": ['hello = hello.hello:main']
        },
    version = version,
    description = "Python command line example package",
    long_description = long_descr,
    author = "Oji Setyawan",
    author_email = "ojixzzz@gmail.com",
    url = "https://github.com/ojixzzz/example-cli-python",
    )
