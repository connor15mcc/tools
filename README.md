## tools

tools: a grab bag of useful tools + utilities, sharpened with age

the main tools binary employs a busybox-style dispatch based on argv0. To
install symlinks for each of the commands:

```
cargo install --path=. && tools --install
```
