# cmdchatter
a prototype cmd line chat app, using holochain, written in Rust

![](./images/cli.png)

## Installing and running

> Prereq: have a [stable version of rust](https://www.rust-lang.org/tools/install) installed

```bash
cargo install --git https://github.com/Sprillow/cmdchatter
cd /a/directory/to/install/persist/and/run/cmdchatter
RUST_LOG=none cmdchatter
```

> If you don't set RUST_LOG=none you will get a flood of irrelevant holochain networking error logs

Next time you want to use it, make sure you `cd` into the same directory
as you initialized in.