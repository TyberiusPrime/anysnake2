#!/bin/bash
cd examples/dev || true
fd "rs|py|nix|toml" ../../| entr $@
