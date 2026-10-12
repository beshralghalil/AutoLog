<div align="center">
  <a href="https://github.com/BishrGhalil/AutoLog">
    <img alt="AutoLog.Logo" width="200" height="200" src="./assets/icon.png">
  </a>
  <h1>AutoLog</h1>
  <p>
    automates the process of logging work entries into Jira and Kimai.
  </p>
  <a href="https://github.com/BishrGhalil/AutoLog/releases/latest">
    <img src="https://img.shields.io/github/v/release/BishrGhalil/AutoLog">
  </a>
  <a href="https://github.com/BishrGhalil/AutoLog/releases/latest">
    <img src="https://img.shields.io/github/release-date/BishrGhalil/AutoLog">
  </a>
  <a href="https://github.com/BishrGhalil/AutoLog/tree/dev">
    <img src="https://img.shields.io/github/last-commit/BishrGhalil/AutoLog">
  </a>
  <a href="https://github.com/BishrGhalil/AutoLog/releases/">
    <img src="https://img.shields.io/github/downloads/BishrGhalil/AutoLog/total">
  <a href="https://github.com/BishrGhalil/AutoLog/blob/dev/LICENSE">
    <img src="https://img.shields.io/github/license/BishrGhalil/AutoLog">
  </a>
</div>


## About
**AutoLog** reads a worklog export from wherever you track your time, and posts every entry to wherever it has to be logged.

It is built around two sides:

- **Providers** are where the entries come from: a **CSV** or **Excel** export from **Grindstone**, **Kimai**, or **Odoo**.
- **Consumers** are where the entries go: **Jira** or **Kimai**.

Pick one of each, point AutoLog at the file, and review the table before anything is posted.

<img src=".github/ui.png" alt="UI" width="70%"/>

## Prerequisites
- A worklog file exported from one of the supported providers.
- Credentials for the consumer you are logging into:
  - **Jira**: your account email and an [API token](https://support.atlassian.com/atlassian-account/docs/manage-api-tokens-for-your-atlassian-account/#Create-an-API-token).
  - **Kimai**: an API token from *User profile → API*, used as a bearer token.

Credentials are stored per consumer in your system keyring, so you can switch between Jira and Kimai without retyping them.

## Logging into Jira
Each entry needs an issue key. AutoLog takes it from the provider's work item or activity field, understanding formats like `ABC-123`, `[ABC-123]` and `[ABC] 123`. Any entry it cannot read a key from is flagged in the table — double-click the **Issue** cell to fix it before processing.

## Logging into Kimai
Kimai files entries under a project and an activity instead of an issue key, so AutoLog adds a **Project** and an **Activity** column.

After the file is loaded, AutoLog fetches your projects and activities and matches each entry against them, using the work item as a hint — an entry on `ACME-6055` lands on the `ACME` project, and `WIDGET-BaseImage` on `WIDGET` / `BaseImage`. Matching is fuzzy, so names do not have to line up exactly.

You stay in control of the result:

- Use the **Project** and **Activity** dropdowns above the file picker to apply one choice to every entry at once.
- Double-click any **Project** or **Activity** cell to override a single entry. Changing a row's project re-matches its activity within that project.

Anything left unmatched is flagged and skipped rather than guessed at.

---

# Usage:

## Download
You may download the executable from the [releases page](https://github.com/BishrGhalil/AutoLog/releases)

## Run from source code
Clone the repository:

```bash
git clone https://github.com/BishrGhalil/AutoLog.git
cd AutoLog
```

Set up your Python environment:

```bash
python -m venv .venv
source .venv/bin/activate
python -m pip install -r requirements.txt
python main.py
```

---

## Development

```bash
python3 -m pip install uv
uv sync
pre-commit install
uv run main.py
```

Use [commitizen](https://commitizen-tools.github.io/commitizen/):

for committing
```bash
cz c
```

for bumping:
```bash
cz bump
```

Run linters:

```bash
ruff check .
```

Build executable:

```bash
pyinstaller --noconfirm --clean autolog.spec
```

[Signing the executable](https://gist.github.com/PaulCreusy/7fade8d5a8026f2228a97d31343b335e)

---

## Roadmap

- [X] Cooldown
- [X] Windows Executable
- [X] Code signing for Windows
- [X] Kimai as a logging target
- [ ] Async requests

---

## Contributing

Contributions are welcome. Please fork the repo and create a pull request.

---

## License

MIT License. See `LICENSE` file for details.
