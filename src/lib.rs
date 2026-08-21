//! # fints — Native Rust FinTS 3.0 PinTan Client
//!
//! A pure Rust implementation of the FinTS 3.0 (formerly HBCI) banking protocol
//! for German online banking.
//!
//! ## Architecture
//!
//! 1. **Protocol layer** (`protocol`): Typestate `Dialog<S>` — the dialog's auth
//!    state is in the type system. Business ops on an unauthenticated dialog = compile error.
//!
//! 2. **Workflow layer** (`workflow`): Bank-specific workflows via `BankOps` trait.
//!
//! 3. **Bank modules** (`dkb`): High-level, bank-specific APIs.
//!
//! ## DKB — Quick start
//!
//! ```rust,no_run
//! use fints::{dkb, Account, UserId, Pin, ProductId};
//!
//! # async fn example() -> fints::Result<()> {
//! let (session, challenge) = dkb::connect(
//!     &UserId::new("user"), &Pin::new("pin"), &ProductId::new("PRODUCT_ID"), None,
//! ).await?;
//! // User confirms pushTAN in banking app...
//! let account = Account::new("DE123...", "BYLADEM1001")?;  // BIC required!
//! let data = session.fetch(&account, 365).await?;
//! println!("Balance: {:?}, {} transactions", data.balance, data.transactions.len());
//! # Ok(())
//! # }
//! ```
//!
//! ## Generic bank access
//!
//! ```rust,no_run
//! use fints::{Flow, UserId, Pin, ProductId};
//!
//! # async fn example() -> fints::Result<()> {
//! let (mut flow, challenge) = Flow::initiate(
//!     "12030000", &UserId::new("user"), &Pin::new("pin"), &ProductId::new("PRODUCT_ID"),
//!     None, None, None,
//! ).await?;
//! let result = flow.confirm_and_fetch("DE123...", "BYLADEM...", 365).await?;
//! # Ok(())
//! # }
//! ```

// ── Infrastructure ──
pub mod banks;
pub mod banks_generated;
pub mod error;
pub(crate) mod message;
pub mod parser;
pub(crate) mod segments;
pub(crate) mod serializer;
pub(crate) mod transport;
pub mod types;

// ── Tooling ──
pub mod audit;
pub mod debug;

// ── Architecture ──
pub mod flow;
pub mod protocol;
pub mod workflow;

// ── Bank APIs ──
pub mod dkb;

// ═══════════════════════════════════════════════════════════════════════════════
// Re-exports
// ═══════════════════════════════════════════════════════════════════════════════

// Flow layer
pub use flow::{ChallengeInfo, FetchOptions, Flow, SyncResult};

// Workflow layer
pub use workflow::{bank_ops, bank_ops_with_config, AnyBank, BankOps, Dkb, GenericBank};
pub use workflow::{FetchOpts, FetchResult, InitiateNoTanResult, InitiateOutcome, InitiateResult};

// Protocol layer
pub use protocol::{
    Account, BalanceResult, BankParams, CreditCardTransactionPage, CreditCardTransactionResult,
    DepotHistoryResult, DepotHistorySnapshot, DepotTransactionPage, DepotTransactionResult, Dialog,
    HoldingsPage, HoldingsResult, InitResult, New, Open, PollResult, Response, SendResult, Synced,
    TanChallenge, TanPending, TransactionPage, TransactionResult,
};

// Domain types
pub use banks::{all_banks, bank_by_blz, BankConfig};
pub use error::{FinTSError, Result};
pub use types::{
    AccountBalance, BankName, Bic, Blz, ChallengeText, Currency, DepotTransaction,
    DepotTransactionDirection, DialogId, FinTSUrl, HhdUcData, Iban, Isin, Mt940Data, Pin,
    ProductId, ResponseCode, ResponseCodeKind, SecurityFunction, SecurityHolding, SecurityReference,
    SegmentRef, SegmentType, SepaAccount, SystemId, TanMediumName, TanMethod, TanProcess,
    TaskReference, TouchdownPoint, Transaction, TransactionStatus, UserId, Wkn,
};

// Debug / audit tooling
pub use audit::{
    audit_client_message, audit_server_response, AuditReport, Violation, ViolationSeverity,
};
pub use debug::{
    decode_message, format_decoded, serialize_raw_segment, DecodedMessage, DecodedSegment,
    VerbosityLevel,
};
