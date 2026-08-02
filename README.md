# fints-rs

[![crates.io](https://img.shields.io/crates/v/fints-rs.svg)](https://crates.io/crates/fints-rs)
[![docs.rs](https://docs.rs/fints-rs/badge.svg)](https://docs.rs/fints-rs)

A pure Rust implementation of the **FinTS 3.0** (formerly HBCI) banking protocol for German online banking.

## Features

- Full FinTS 3.0 PinTan implementation (two-step and decoupled TAN)
- Typestate protocol layer — invalid dialog state transitions are **compile-time errors**
- Bank-specific workflows (DKB, generic FinTS banks)
- Built-in registry of 1000+ German banks with FinTS endpoints
- Account balance, transaction history (MT940), and securities/depot holdings
- CLI client for interactive banking from the terminal
- Mock server for integration testing
- Debug & audit tooling for protocol inspection

## Installation

### As a library

Add to your `Cargo.toml`:

```toml
[dependencies]
fints-rs = "0.1"
```

The crate is published as `fints-rs` on crates.io but the library name is `fints`, so all imports use `use fints::...`.

### CLI client

```bash
cargo install fints-rs --features cli
```

This installs the `fints-client` binary.

## Library Usage

### Quick start — DKB

```rust
use fints::{dkb, Account, UserId, Pin, ProductId};

#[tokio::main]
async fn main() -> fints::Result<()> {
    // Step 1: Connect and get TAN challenge
    let (session, challenge) = dkb::connect(
        &UserId::new("your_user_id"),
        &Pin::new("your_pin"),
        &ProductId::new("YOUR_PRODUCT_ID"),
        None,
    ).await?;

    println!("Please confirm in your banking app: {}", challenge.challenge);

    // Step 2: After user confirms pushTAN, fetch data
    let account = Account::new("DE12345678901234567890", "BYLADEM1001")?;
    let result = session.fetch(&account, 365).await?;

    println!("Balance: {:?}", result.balance);
    println!("{} transactions", result.transactions.len());
    Ok(())
}
```

### Generic bank access (any FinTS bank)

```rust
use fints::{Flow, UserId, Pin, ProductId};

#[tokio::main]
async fn main() -> fints::Result<()> {
    // Step 1: Initiate — pass BLZ to auto-resolve the bank
    let (mut flow, challenge) = Flow::initiate(
        "12030000",  // BLZ (bank code)
        &UserId::new("your_user_id"),
        &Pin::new("your_pin"),
        &ProductId::new("YOUR_PRODUCT_ID"),
        None, None, None,
    ).await?;

    println!("TAN challenge: {}", challenge.challenge);

    // Step 2: After TAN confirmation, fetch balance + transactions
    let result = flow.confirm_and_fetch(
        "DE12345678901234567890",  // IBAN
        "BYLADEM1001",             // BIC
        90,                        // days of history
    ).await?;

    println!("Balance: {:?}", result.balance);
    println!("{} transactions", result.transactions.len());
    println!("{} holdings", result.holdings.len());
    Ok(())
}
```

### Bank lookup

```rust
use fints::{all_banks, bank_by_blz};

// Look up a bank by BLZ
if let Some(bank) = bank_by_blz("12030000") {
    println!("{} — {}", bank.name, bank.url);
}

// List all known banks
for bank in all_banks() {
    println!("{}: {} ({})", bank.blz, bank.name, bank.url);
}
```

### Low-level protocol access

The typestate `Dialog` API enforces correct protocol usage at compile time:

```rust
use fints::protocol::Dialog;
use fints::types::*;

// Dialog<New> → sync → Dialog<Synced> → open → Dialog<Open> → business ops
// Calling business methods on Dialog<New> won't compile.
```

States: `New` → `Synced` → `Open` → `TanPending` → back to `Open` after TAN.

## CLI Usage

### First-time setup

```bash
fints-client --bank dkb setup
# or by BLZ:
fints-client --bank 12030000 setup
```

This performs a sync dialog, discovers accounts and TAN methods, and saves a session file for future use.

### Sync (balance + transactions + holdings)

```bash
fints-client --bank dkb sync
fints-client --bank dkb sync --days 365
fints-client --bank dkb sync --all-accounts
```

### Balance only

```bash
fints-client --bank dkb balance
fints-client --bank dkb balance --iban DE12345678901234567890
```

### Transactions

```bash
fints-client --bank dkb transactions
fints-client --bank dkb transactions --from 2024-01-01 --to 2024-12-31
fints-client --bank dkb transactions --output json
fints-client --bank dkb transactions --output csv
```

### Holdings (securities/depot)

```bash
fints-client --bank dkb holdings
```

### Custom bank (by URL)

```bash
fints-client --bank custom --url https://banking.example.de/fints --blz 12345678 sync
```

### Session management

```bash
fints-client sessions list
fints-client sessions inspect dkb
fints-client sessions delete dkb
```

### Debug & audit tools

```bash
# Inspect bank capabilities
fints-client --bank dkb inspect

# Decode a raw FinTS message
echo "<base64_message>" | fints-client decode --b64

# Audit a bank's protocol compliance
fints-client audit --blz 12030000
```

### Output formats

All data commands support `--output human` (default), `--output json`, and `--output csv`.

### Environment variables

| Variable | Description |
|---|---|
| `FINTS_PRODUCT_ID` | Override the default FinTS product registration ID |
| `FINTS_SESSION_DIR` | Override the session storage directory |

### Global options

```
--bank <BLZ|name>     Bank identifier (BLZ or shorthand like "dkb")
--user <USER_ID>      FinTS user ID
--pin <PIN>           PIN (prefer interactive prompt instead)
--product-id <ID>     FinTS product registration ID
--verbose / -v        Show decoded FinTS segments
--debug-wire          Show hex wire dumps
--no-persist          Don't save session; print resume token instead
--resume-token <TOK>  Resume from a previously printed token
```

## Architecture

```
┌─────────────────────────────────────────────────┐
│                    Flow API                     │  ← High-level 2-step TAN flow
├─────────────────────────────────────────────────┤
│                 Workflow layer                  │  ← Bank-specific logic (DKB, Generic)
│            BankOps trait + AnyBank              │
├─────────────────────────────────────────────────┤
│               Protocol layer                   │  ← Typestate Dialog<S> state machine
│     Dialog<New|Synced|Open|TanPending>          │
├──────────────────┬──────────────────────────────┤
│    Segments      │     Message builder          │  ← HKSAL, HKKAZ, HKTAN, ...
├──────────────────┼──────────────────────────────┤
│  Parser          │     Serializer               │  ← Wire format (DEG/DE encoding)
├──────────────────┴──────────────────────────────┤
│              Transport (HTTPS)                  │  ← Base64 over HTTPS POST
└─────────────────────────────────────────────────┘
```

## Feature flags

| Feature | Description |
|---|---|
| `cli` | Enables the `fints-client` binary (clap, rpassword, comfy-table, csv, flate2) |
| `server` | Enables the `fints-server` mock server binary (axum) |

No features are enabled by default — the library is lightweight with minimal dependencies.

## FinTS Product Registration

To use FinTS with German banks in production, you need a registered product ID from the [Deutsche Kreditwirtschaft](https://www.fints.org/de/hersteller/produktregistrierung). Set it via:

- `ProductId::new("YOUR_ID")` in code
- `--product-id YOUR_ID` on the CLI
- `FINTS_PRODUCT_ID=YOUR_ID` environment variable

## License

MIT
