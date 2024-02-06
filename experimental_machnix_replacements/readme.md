# Replace mach-nix


Mach-nix is great.
Mach-nix is a constant source of pain.

There are two parts to mach-nix,
both somewhat complicated:

1. Python package resolution as as if it was <date>
2. Turning those resolved packages into nix.

I think, poetry2nix is ready to take on part 2.
And it works without the nix-pypi-fetcher(-2) database
for the hashes, since it relies on those in the lockfiles.

Part 1 has been plagued by pypi-deps-db issues. It's 
not been updating for three months, it's archived,
pypi-deps-db(1) is giant, and just using it involves
downloading hundreds of megabytes. 
For each date.


In this I want to explore an alternate approach:
Proxy pypi, and filter packages that were published
after <date>. The proof-of-concept in proxy.py is rather simple.

And all it needs is to add
```
[[tool.poetry.source]]
name = "proxy"
url = "http://localhost:8080/simple"
priority = "primary"


[[tool.poetry.source]]
name = "PyPI"
priority = "explicit"
```
to pyproject.toml, and then run poetry on it.

