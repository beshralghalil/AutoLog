<div align="center">
  <a href="https://github.com/BishrGhalil/AutoLog">
    <img alt="AutoLog logo" width="200" height="200" src="./assets/icon.png">
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
  <a href="https://github.com/BishrGhalil/AutoLog/releases/">
    <img src="https://img.shields.io/github/downloads/BishrGhalil/AutoLog/total">
  </a>
  <a href="https://github.com/BishrGhalil/AutoLog/blob/main/LICENSE">
    <img src="https://img.shields.io/github/license/BishrGhalil/AutoLog">
  </a>
</div>

## About
**AutoLog** reads a worklog export from wherever you track your time, and posts every entry to wherever it has to be logged.

It is built around two sides:

- **Providers** are where the entries come from: a **CSV** or **spreadsheet** export from **Grindstone**, **Kimai**, or **Odoo**.
- **Consumers** are where the entries go: **Jira** or **Kimai**.

Pick one of each, point AutoLog at the file, and review the table before anything is posted.

<img src=".github/ui.png" alt="UI" width="70%"/>

## Prerequisites
- A worklog file exported from one of the supported providers.
- Credentials for the consumer you are logging into:
  - **Jira**: your site address, your account email and an [API token](https://support.atlassian.com/atlassian-account/docs/manage-api-tokens-for-your-atlassian-account/#Create-an-API-token).
  - **Kimai**: your site address and an API token from *User profile → API*.

Tokens are stored per consumer in your system keyring, so you can switch between Jira and Kimai without retyping them. Only addresses and emails are written to the settings file.

## Logging into Jira
Each entry needs an issue key. AutoLog reads one from the provider's work item, understanding `ABC-123`, `[ABC-123]` and `[ABC] 123`.

On connect it also fetches your projects, and the issues you are assigned to or have already logged against, and matches each entry to them. Pick a different one from any **Project** or **Issue** cell; an issue the search did not return can be typed straight into the picker.

## Logging into Kimai
Kimai files entries under a project and an activity instead of an issue key, so AutoLog adds a **Project** and an **Activity** column.

After the file is loaded, AutoLog fetches your projects and activities and matches each entry against them, using the work item as a hint — an entry on `ACME-6055` lands on the `ACME` project. Activities are narrowed to the project you chose, so changing a row's project re-matches its activity underneath.

Names that differ only in their numbers are never paired up, so an entry on `ABC-6061` will not quietly land on an `ABC-6055` activity. Anything left unmatched keeps the text it was matched against, coloured so it stands out, and is refused at processing time rather than guessed at.

You stay in control of the result:

- Use the pickers in the toolbar to apply one choice to every entry at once, or to just the rows you have ticked.
- Click any cell to override a single entry.
- **Refresh** re-fetches everything from the tracker, filling only the gaps, so anything you mapped by hand is kept.
- How hard AutoLog guesses is set per field in **Settings** — exact, contains, similar, or not at all.

## Posting
**Dry run** checks every entry against the tracker without sending anything.

**Post** sends everything not already posted, several at a time under a rate limit you can set. A failed row can be fixed and posted again; anything already posted is left alone. With duplicate checking on — the toggle sits on the bottom bar — AutoLog skips entries the tracker already has.

---

# Usage:

## Download
You may download the executable for your platform from the [releases page](https://github.com/BishrGhalil/AutoLog/releases). There is nothing else to install.

You can also open an export with it directly: `autolog path/to/export.csv`.

## Run from source code
Clone the repository:

```bash
git clone https://github.com/BishrGhalil/AutoLog.git
cd AutoLog
```

Then:

```bash
cargo run -p autolog-gui
```

---

## Development

```bash
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --all
```

Two crates: **`autolog-core`** holds the rules and the tests, with no knowledge of a window; **`autolog-gui`** is the window.

Adding a **provider** means adding a `ProviderSpec` to [`source/spec.rs`](crates/autolog-core/src/source/spec.rs), naming its columns and how it writes a duration. There is no trait to implement.

Adding a **consumer** means a `ConsumerSpec` declaring the credentials and destinations it needs, plus the [`Consumer`](crates/autolog-core/src/consumer/mod.rs) calls that are its own: signing in, listing its names, reading back what is already logged, and posting. Matching, bulk editing and its settings come from the spec.

If credentials are not being remembered, this reports what your system's credential store is doing, without printing any values:

```bash
cargo run -p autolog-core --example keyring_check
```

Build a release executable:

```bash
cargo build --release -p autolog-gui
```

[Signing the executable](https://gist.github.com/PaulCreusy/7fade8d5a8026f2228a97d31343b335e)

---

## Roadmap

- [X] Cooldown
- [X] Windows Executable
- [X] Code signing for Windows
- [X] Kimai as a logging target
- [X] Async requests
- [ ] A command-line mode for unattended runs

---

## Contributing

Contributions are welcome. Please fork the repo and create a pull request.

---

## License

MIT License. See `LICENSE` file for details.
