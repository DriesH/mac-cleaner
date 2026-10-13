# mac-cleaner

Find and clean caches, build folders and other junk that fills up your Mac.

## Install

```sh
cargo install --path .
```

Give your terminal **Full Disk Access** (System Settings > Privacy & Security) to also scan the Trash and iPhone backups.

## Use

```sh
mac-cleaner                          # interactive cleaner
mac-cleaner scan                     # size per category
mac-cleaner scan -c project-deps -i  # list every node_modules, target, ...
mac-cleaner clean user-caches logs   # clean categories, asks first
mac-cleaner clean project-deps --older-than 90 --projects-dir ~/code
mac-cleaner categories               # list category names
```

By default, items go to the **Trash**. Empty the Trash to get the space back. Use `--permanent` (or press `p` in the app) to delete for good.

## Keys

| Key | Action |
| --- | --- |
| `↑ ↓` / `j k` | move |
| `← →` / `tab` / `enter` | switch between categories and items |
| `space` | select item, or whole category |
| `a` / `c` | select all in category / clear selection |
| `d` | clean selected items |
| `p` | switch between Trash and permanent delete |
| `s` | sort by size, age or name |
| `o` | show in Finder |
| `r` | scan again |
| `?` | help |
