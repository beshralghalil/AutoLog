## 3.0.0 (2026-10-14)

Rewritten in Rust. A single executable for Windows and Linux, with nothing to install alongside it.

### Feat

- **ui**: new window built on egui, with a virtualised table that stays smooth on large exports
- **ui**: show every column the source file has, with a per-provider column picker
- **ui**: search across every column, filter by status, sort by any column
- **ui**: select rows and set a destination on all of them at once
- **ui**: pick destinations in the cell itself, from a searchable list
- **ui**: light and dark themes, following the desktop by default
- **matching**: choose the strategy per destination field - manual, exact, contains or similar
- **matching**: Kimai projects match exactly by default; activities stay loose
- **consumers**: Jira fetches its projects on connect, so a mistyped key is caught before posting
- **consumers**: choosing a Jira project rewrites that entry's issue key
- **posting**: post entries in parallel, under a configurable rate limit
- **posting**: dry run checks every entry without sending anything
- **posting**: stop a run between entries
- **config**: every choice persists between sessions, including the selected consumer
- **providers**: read .tsv and .ods as well as .csv, .xlsx, .xlsm and .xls
- **providers**: sniff comma, semicolon and tab separators
- **cli**: open an export by passing its path, so AutoLog can be the program a file opens with

### Fix

- **providers**: Odoo entries longer than a day no longer wrap to under an hour
- **providers**: rows that cannot be read are listed rather than silently dropped
- **duplicates**: compare starts to the minute, as was intended, rather than to the second
- **net**: never retry a post after a timeout, which could log the same work twice
- **net**: honour Retry-After, and back off with jitter on rate limits and 5xx
- **security**: only tokens go to the system credential store; nothing secret reaches the config file

## 2.1.1 (2025-05-25)

### Fix

- fix version definition module

## 2.1.0 (2025-05-25)

### Feat

- add update checker

## 2.0.0 (2025-05-25)

### Feat

- **logging**: add console handler
- add support for different providers
- add commitizen integration

## v1.1.0 (2025-05-21)

### Feat

- updated show_error with logfile info
- improved datetime parsing
- improved logging path

### Fix

- match date not only time to detect duplicates
- only create tag if not exists

## v1.0.1 (2025-05-19)

### Feat

- added icon, build workflow

### Fix

- deprecated set-output in actions
- replaced workflow signing logic
- fixed missing run command in workfow
- add find-singtool to path in workflow
- install uv in workflow
- removed version param from workflow dispatch
- build workflow toml parsing

## v1.0.0 (2025-05-11)

### Feat

- add ui screenshot
- refactor, support for duplicates handling
- improved progressbar and status coloring and updating
- improved logging
- status bar text
- exception logging
- add cooldown
- performance improvment by not fetching the issue first
- tree view styling
- pyinstaller support for building executables

### Fix

- set a timeout for jira client
- not updating entries after first n
- align table text to center
- removed wrong datas from spec
- fixed typo in readm

### Refactor

- issue key parsing
- parse issue keys in main app
