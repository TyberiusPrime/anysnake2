#!/bin/bash
cd example
fd "rs|py|nix|toml" ../| entr $@
