#!/bin/bash

cd dna/cmdchat
cargo build --release --target wasm32-unknown-unknown
cd ..
dna-util -c cmdchat.dna.workdir
cd ..