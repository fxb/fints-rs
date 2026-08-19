//! Bank workflow trait and DKB implementation.
//!
//! Each bank defines its own complete workflow via the `BankOps` trait.
//! The workflows compose typed Dialog transitions from `protocol.rs`.
//!
//! Compile-time safety: workflow methods take typed dialog states.
//! `fetch()` takes `Dialog<Open>` — you can't call it without authentication.

use tracing::{info, warn};

use crate::banks::BankConfig;
use crate::error::{FinTSError, Result};
use crate::protocol::*;
use crate::types::*;

// ═══════════════════════════════════════════════════════════════════════════════
// Workflow result types
// ═══════════════════════════════════════════════════════════════════════════════

/// Result of initiating a connection.
pub struct InitiateResult {
    pub dialog: Dialog<TanPending>,
    pub challenge: TanChallenge,
    pub tan_methods: Vec<TanMethod>,
    pub allowed_security_functions: Vec<SecurityFunction>,
    pub no_tan_required: bool,
    pub params: BankParams,
    pub system_id: SystemId,
}

/// Result when no TAN is required (SCA exemption).
pub struct InitiateNoTanResult {
    pub dialog: Dialog<Open>,
    pub params: BankParams,
    pub system_id: SystemId,
    pub tan_methods: Vec<TanMethod>,
    pub allowed_security_functions: Vec<SecurityFunction>,
}

/// Either we need TAN or we're already authenticated.
pub enum InitiateOutcome {
    NeedTan(InitiateResult),
    Authenticated(InitiateNoTanResult),
}

/// Result of fetching data from an open dialog.
pub struct FetchResult {
    pub balance: Option<AccountBalance>,
    pub transactions: Vec<Transaction>,
    pub holdings: Vec<SecurityHolding>,
}

/// Options controlling what data to fetch in a single authenticated dialog.
#[derive(Debug, Clone, Default)]
pub struct FetchOpts {
    /// Fetch balance (HKSAL). Default: true.
    pub balance: bool,
    /// Fetch transactions (HKKAZ). Default: true.
    pub transactions: bool,
    /// Fetch securities holdings (HKWPD). Default: true.
    pub holdings: bool,
    /// Fetch credit card transactions (DKKKU). Requires `credit_card_number`.
    pub credit_card: bool,
    /// Credit card number (PAN) for DKKKU requests.
    pub credit_card_number: Option<String>,
    /// Days of transaction history to fetch. Default: 90.
    pub days: u32,
}

impl FetchOpts {
    /// Fetch everything: balance, transactions, and holdings.
    pub fn all(days: u32) -> Self {
        Self { balance: true, transactions: true, holdings: true, days, ..Default::default() }
    }
    /// Fetch only balance (single request, fast).
    pub fn balance_only() -> Self {
        Self { balance: true, ..Default::default() }
    }
    /// Skip holdings (for accounts without a depot).
    pub fn no_holdings(days: u32) -> Self {
        Self { balance: true, transactions: true, days, ..Default::default() }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// BankOps trait
// ═══════════════════════════════════════════════════════════════════════════════

/// Each bank implements its own workflow as typed Dialog transitions.
///
/// `fetch()` takes `&mut Dialog<Open>` and `&Account` — compile-time proof that:
/// 1. Authentication has been completed (Dialog<Open>)
/// 2. Account has valid IBAN + BIC (Account)
pub trait BankOps: Send + Sync {
    fn config(&self) -> &BankConfig;

    /// Phase 1: sync + init, return TAN challenge or authenticated dialog.
    fn initiate(
        &self,
        username: &UserId,
        pin: &Pin,
        product_id: &ProductId,
        system_id: Option<&SystemId>,
        target_iban: Option<&Iban>,
        target_bic: Option<&Bic>,
    ) -> impl std::future::Future<Output = Result<InitiateOutcome>> + Send;

    /// Phase 2: fetch balance + transactions from an open dialog.
    /// Takes `&Account` — IBAN and BIC are guaranteed present.
    fn fetch(
        &self,
        dialog: &mut Dialog<Open>,
        account: &Account,
        days: u32,
    ) -> impl std::future::Future<Output = Result<FetchResult>> + Send;

    /// Fetch securities holdings from an open dialog.
    /// Takes `&Account` — IBAN and BIC are guaranteed present.
    /// Returns an empty Vec if the bank does not support depot queries.
    fn fetch_holdings(
        &self,
        dialog: &mut Dialog<Open>,
        account: &Account,
    ) -> impl std::future::Future<Output = Result<Vec<SecurityHolding>>> + Send;
}

// ═══════════════════════════════════════════════════════════════════════════════
// Shared: resume with a stored system_id (skip HKSYN)
// ═══════════════════════════════════════════════════════════════════════════════

/// Try to initiate directly with a stored system_id, skipping the sync dialog.
///
/// HKSYN mode 0 always requests a NEW system id, so running the sync dialog on
/// every connection makes the bank see a fresh "device" each time — defeating
/// device recognition and any SCA exemption (code 3076). When the caller
/// passes a previously assigned system_id, initialize directly with it instead;
/// BPD and the security function are negotiated inside `init_negotiate()`.
///
/// Returns `None` if the resume attempt failed (caller should fall back to the
/// full sync flow, which registers a new system id).
async fn try_resume_init(
    bank: &BankConfig,
    username: &UserId,
    pin: &Pin,
    product_id: &ProductId,
    system_id: &SystemId,
) -> Option<InitiateOutcome> {
    let dialog = match Dialog::new(bank.url.as_str(), &bank.blz, username, pin, product_id) {
        Ok(d) => d.with_system_id(system_id),
        Err(e) => {
            warn!("[FinTS] Resume: dialog setup failed: {e}");
            return None;
        }
    };

    match dialog.init_negotiate().await {
        Ok(InitResult::TanRequired(tan_pending, challenge, _resp)) => {
            info!("[FinTS] Resume with stored system_id: TAN required (decoupled={})",
                challenge.decoupled);
            let params = tan_pending.bank_params().clone();
            Some(InitiateOutcome::NeedTan(InitiateResult {
                system_id: tan_pending.system_id().clone(),
                tan_methods: params.tan_methods.clone(),
                allowed_security_functions: params.allowed_security_functions.clone(),
                params,
                dialog: tan_pending,
                challenge,
                no_tan_required: false,
            }))
        }
        Ok(InitResult::Opened(open, _resp)) => {
            info!("[FinTS] Resume with stored system_id: opened without TAN (SCA exemption)");
            let params = open.bank_params().clone();
            Some(InitiateOutcome::Authenticated(InitiateNoTanResult {
                system_id: open.system_id().clone(),
                tan_methods: params.tan_methods.clone(),
                allowed_security_functions: params.allowed_security_functions.clone(),
                params,
                dialog: open,
            }))
        }
        Err(e) => {
            warn!("[FinTS] Resume with stored system_id failed ({e}); \
                   falling back to full sync (new system id)");
            None
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// DKB implementation
// ═══════════════════════════════════════════════════════════════════════════════

/// DKB (Deutsche Kreditbank) FinTS workflow.
///
/// DKB message flow per spec + empirical discovery:
/// ```text
///   Sync dialog:
///     Msg 1: HKIDN + HKVVB + HKSYN   → BPD, system_id
///     Msg 2: HKEND
///
///   Business dialog:
///     Msg 1: HKIDN + HKVVB + HKTAN:4(ref=HKIDN)  → InitResult
///       → TanRequired: push sent (3955)
///       → Opened: SCA exemption (3076)
///     Msg 2: HKTAN:S(task_ref)                     → PollResult
///       → Confirmed: 0020
///       → Pending: 3955/3956
///     Msg 3: HKSAL [+ HKTAN:4(ref=HKSAL)]         → SendResult
///       → Success: balance data (HISAL)
///       → NeedTan: additional TAN for balance
///     Msg 4: HKKAZ [+ HKTAN:4(ref=HKKAZ)]         → SendResult
///       → Success: transaction data (HIKAZ)
///       → Touchdown: more data, fetch again
///     Msg 5: HKEND
/// ```
pub struct Dkb {
    bank: BankConfig,
}

impl Dkb {
    pub fn new() -> Self {
        Self {
            bank: crate::banks::bank_by_blz("12030000")
                .expect("DKB (BLZ 12030000) must be in bank registry"),
        }
    }

    fn new_dialog(&self, username: &UserId, pin: &Pin, product_id: &ProductId) -> Result<Dialog<New>> {
        Dialog::new(
            self.bank.url.as_str(),
            &self.bank.blz,
            username,
            pin,
            product_id,
        )
    }
}

impl BankOps for Dkb {
    fn config(&self) -> &BankConfig { &self.bank }

    async fn initiate(
        &self,
        username: &UserId,
        pin: &Pin,
        product_id: &ProductId,
        system_id: Option<&SystemId>,
        _target_iban: Option<&Iban>,
        _target_bic: Option<&Bic>,
    ) -> Result<InitiateOutcome> {
        // ── Phase 0: Resume with stored system_id (skip HKSYN) ──
        if let Some(sid) = system_id.filter(|s| s.is_assigned()) {
            if let Some(outcome) =
                try_resume_init(&self.bank, username, pin, product_id, sid).await
            {
                return Ok(outcome);
            }
        }

        // ── Phase 1: Sync dialog (get system_id + BPD) ──
        let sync_dialog = self.new_dialog(username, pin, product_id)?;
        let (synced, _resp) = sync_dialog.sync().await?;
        let (sync_params, sys_id) = synced.end().await?;

        let sys_id = if sys_id.is_assigned() {
            sys_id
        } else {
            system_id.cloned().unwrap_or_else(SystemId::unassigned)
        };

        // ── Phase 2: Normal dialog init (triggers TAN or opens directly) ──
        let dialog = self.new_dialog(username, pin, product_id)?
            .with_system_id(&sys_id)
            .with_params(&sync_params);

        let init_result = dialog.init().await?;

        match init_result {
            InitResult::TanRequired(tan_pending, challenge, _resp) => {
                info!("[DKB] TAN required: decoupled={}, task_ref='{}'",
                    challenge.decoupled, challenge.task_reference);
                Ok(InitiateOutcome::NeedTan(InitiateResult {
                    params: tan_pending.bank_params().clone(),
                    system_id: tan_pending.system_id().clone(),
                    dialog: tan_pending,
                    challenge,
                    tan_methods: sync_params.tan_methods.clone(),
                    allowed_security_functions: sync_params.allowed_security_functions.clone(),
                    no_tan_required: false,
                }))
            }
            InitResult::Opened(open, _resp) => {
                info!("[DKB] Opened directly (SCA exemption)");
                Ok(InitiateOutcome::Authenticated(InitiateNoTanResult {
                    params: open.bank_params().clone(),
                    system_id: open.system_id().clone(),
                    dialog: open,
                    tan_methods: sync_params.tan_methods.clone(),
                    allowed_security_functions: sync_params.allowed_security_functions.clone(),
                }))
            }
        }
    }

    async fn fetch(
        &self,
        dialog: &mut Dialog<Open>,
        account: &Account,
        days: u32,
    ) -> Result<FetchResult> {
        info!("[DKB] Fetching IBAN={}, BIC={}", account.iban(), account.bic());

        // ── Balance (HKSAL) ──
        let balance = match dialog.balance(account).await {
            Ok(BalanceResult::Success(b)) => {
                info!("[DKB] Balance: {}", b.amount);
                Some(b)
            }
            Ok(BalanceResult::NeedTan(_)) => {
                warn!("[DKB] Balance requires additional TAN — skipping");
                None
            }
            Ok(BalanceResult::Empty) => {
                warn!("[DKB] No balance data in response");
                None
            }
            Err(e) => {
                warn!("[DKB] Balance failed: {}", e);
                None
            }
        };

        // ── Transactions (HKKAZ) with pagination ──
        let end_date = chrono::Utc::now().date_naive();
        let start_date = end_date - chrono::Duration::days(days as i64);
        info!("[DKB] Transactions {} to {}", start_date, end_date);

        let mut all_booked = Mt940Data::new();
        let mut all_pending = Mt940Data::new();
        let mut touchdown: Option<TouchdownPoint> = None;

        loop {
            let result = dialog.transactions(
                account, start_date, end_date, touchdown.as_ref(),
            ).await?;

            match result {
                TransactionResult::NeedTan(_) => {
                    return Err(FinTSError::Dialog(
                        "DKB erfordert für Transaktionen eine weitere TAN-Freigabe.".into()
                    ));
                }
                TransactionResult::Success(page) => {
                    if !page.booked.is_empty() { all_booked.extend(page.booked.0); }
                    if !page.pending.is_empty() { all_pending.extend(page.pending.0); }
                    touchdown = page.touchdown;
                    if touchdown.is_none() { break; }
                    info!("[DKB] Touchdown: more data...");
                }
            }
        }

        let mut transactions = parse_mt940(all_booked.as_bytes(), TransactionStatus::Booked)?;
        if !all_pending.is_empty() {
            transactions.extend(parse_mt940(all_pending.as_bytes(), TransactionStatus::Pending)?);
        }
        info!("[DKB] {} transactions", transactions.len());

        // ── Holdings (HKWPD) — best-effort, non-fatal ──
        let holdings = match self.fetch_holdings(dialog, account).await {
            Ok(h) => {
                info!("[DKB] {} holdings", h.len());
                h
            }
            Err(e) => {
                warn!("[DKB] Holdings fetch failed (non-fatal): {}", e);
                Vec::new()
            }
        };

        Ok(FetchResult { balance, transactions, holdings })
    }

    async fn fetch_holdings(
        &self,
        dialog: &mut Dialog<Open>,
        account: &Account,
    ) -> Result<Vec<SecurityHolding>> {
        info!("[DKB] Fetching holdings IBAN={}, BIC={}", account.iban(), account.bic());

        let mut all_holdings = Vec::new();
        let mut touchdown: Option<TouchdownPoint> = None;

        loop {
            let result = dialog.holdings(
                account, None, touchdown.as_ref(),
            ).await?;

            match result {
                HoldingsResult::NeedTan(_) => {
                    warn!("[DKB] Holdings requires additional TAN — skipping");
                    return Ok(all_holdings);
                }
                HoldingsResult::Empty => {
                    info!("[DKB] No holdings data (depot may be empty or not supported)");
                    break;
                }
                HoldingsResult::Success(page) => {
                    info!("[DKB] Got {} holdings", page.holdings.len());
                    all_holdings.extend(page.holdings);
                    touchdown = page.touchdown;
                    if touchdown.is_none() { break; }
                    info!("[DKB] Holdings touchdown: more data...");
                }
            }
        }

        info!("[DKB] Total: {} holdings", all_holdings.len());
        Ok(all_holdings)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Bank registry
// ═══════════════════════════════════════════════════════════════════════════════

// ═══════════════════════════════════════════════════════════════════════════════
// Generic bank — any FinTS endpoint (used for custom/unknown banks)
// ═══════════════════════════════════════════════════════════════════════════════

/// A generic FinTS bank implementation that works with any BankConfig.
/// Used when the bank ID is not in the registry (e.g. custom URL + BLZ).
pub struct GenericBank {
    bank: BankConfig,
}

impl GenericBank {
    pub fn new(config: BankConfig) -> Self {
        Self { bank: config }
    }

    fn new_dialog(&self, username: &UserId, pin: &Pin, product_id: &ProductId) -> Result<Dialog<New>> {
        Dialog::new(self.bank.url.as_str(), &self.bank.blz, username, pin, product_id)
    }
}

impl BankOps for GenericBank {
    fn config(&self) -> &BankConfig { &self.bank }

    async fn initiate(
        &self,
        username: &UserId,
        pin: &Pin,
        product_id: &ProductId,
        system_id: Option<&SystemId>,
        _target_iban: Option<&Iban>,
        _target_bic: Option<&Bic>,
    ) -> Result<InitiateOutcome> {
        // Resume with stored system_id (skip HKSYN) — see try_resume_init.
        if let Some(sid) = system_id.filter(|s| s.is_assigned()) {
            if let Some(outcome) =
                try_resume_init(&self.bank, username, pin, product_id, sid).await
            {
                return Ok(outcome);
            }
        }

        let sync_dialog = self.new_dialog(username, pin, product_id)?;
        let (synced, _) = sync_dialog.sync().await?;
        let (sync_params, sys_id) = synced.end().await?;

        let sys_id = if sys_id.is_assigned() { sys_id }
            else { system_id.cloned().unwrap_or_else(SystemId::unassigned) };

        let dialog = self.new_dialog(username, pin, product_id)?
            .with_system_id(&sys_id)
            .with_params(&sync_params);

        let init_result = dialog.init().await?;

        match init_result {
            InitResult::TanRequired(tan_pending, challenge, _) => {
                let challenge = crate::protocol::TanChallenge {
                    decoupled: challenge.decoupled || tan_pending.bank_params().is_decoupled(),
                    ..challenge
                };
                Ok(InitiateOutcome::NeedTan(InitiateResult {
                    params: tan_pending.bank_params().clone(),
                    system_id: tan_pending.system_id().clone(),
                    dialog: tan_pending, challenge,
                    tan_methods: sync_params.tan_methods.clone(),
                    allowed_security_functions: sync_params.allowed_security_functions.clone(),
                    no_tan_required: false,
                }))
            }
            InitResult::Opened(open, _) => {
                Ok(InitiateOutcome::Authenticated(InitiateNoTanResult {
                    params: open.bank_params().clone(),
                    system_id: open.system_id().clone(),
                    dialog: open,
                    tan_methods: sync_params.tan_methods.clone(),
                    allowed_security_functions: sync_params.allowed_security_functions.clone(),
                }))
            }
        }
    }

    async fn fetch(&self, dialog: &mut Dialog<Open>, account: &Account, days: u32) -> Result<FetchResult> {
        // Reuse DKB fetch logic (it's generic enough — just uses typed Dialog<Open> methods)
        Dkb::new().fetch(dialog, account, days).await
    }

    async fn fetch_holdings(&self, dialog: &mut Dialog<Open>, account: &Account) -> Result<Vec<SecurityHolding>> {
        Dkb::new().fetch_holdings(dialog, account).await
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Bank registry
// ═══════════════════════════════════════════════════════════════════════════════

/// Enum dispatch for bank implementations — zero-cost, no dynamic dispatch.
///
/// New banks are added here as enum variants. This avoids `Box<dyn BankOps>`
/// which is incompatible with native async fn in traits.
pub enum AnyBank {
    Dkb(Dkb),
    Generic(GenericBank),
}

impl AnyBank {
    pub fn config(&self) -> &BankConfig {
        match self {
            AnyBank::Dkb(b) => b.config(),
            AnyBank::Generic(b) => b.config(),
        }
    }

    pub async fn initiate(
        &self,
        username: &UserId,
        pin: &Pin,
        product_id: &ProductId,
        system_id: Option<&SystemId>,
        target_iban: Option<&Iban>,
        target_bic: Option<&Bic>,
    ) -> Result<InitiateOutcome> {
        match self {
            AnyBank::Dkb(b) => b.initiate(username, pin, product_id, system_id, target_iban, target_bic).await,
            AnyBank::Generic(b) => b.initiate(username, pin, product_id, system_id, target_iban, target_bic).await,
        }
    }

    pub async fn fetch(
        &self,
        dialog: &mut Dialog<Open>,
        account: &Account,
        days: u32,
    ) -> Result<FetchResult> {
        match self {
            AnyBank::Dkb(b) => b.fetch(dialog, account, days).await,
            AnyBank::Generic(b) => b.fetch(dialog, account, days).await,
        }
    }

    pub async fn fetch_holdings(
        &self,
        dialog: &mut Dialog<Open>,
        account: &Account,
    ) -> Result<Vec<SecurityHolding>> {
        match self {
            AnyBank::Dkb(b) => b.fetch_holdings(dialog, account).await,
            AnyBank::Generic(b) => b.fetch_holdings(dialog, account).await,
        }
    }

    /// Fetch data with fine-grained control via `FetchOpts`.
    /// This gives callers a single authenticated dialog for all operations.
    pub async fn fetch_with_opts(
        &self,
        dialog: &mut Dialog<Open>,
        account: &Account,
        opts: &FetchOpts,
    ) -> Result<FetchResult> {
        self.fetch_with_opts_inner(dialog, account, opts, None).await
    }

    /// Like `fetch_with_opts` but with a callback for per-request TAN events.
    /// When a business operation (balance/transactions/holdings) requires TAN,
    /// `on_tan(true)` is called, the TAN is polled until confirmed, then
    /// `on_tan(false)` is called and the operation is retried.
    pub async fn fetch_with_tan_handler(
        &self,
        dialog: &mut Dialog<Open>,
        account: &Account,
        opts: &FetchOpts,
        on_tan: &(dyn Fn(bool) + Send + Sync),
    ) -> Result<FetchResult> {
        self.fetch_with_opts_inner(dialog, account, opts, Some(on_tan)).await
    }

    async fn fetch_with_opts_inner(
        &self,
        dialog: &mut Dialog<Open>,
        account: &Account,
        opts: &FetchOpts,
        on_tan: Option<&(dyn Fn(bool) + Send + Sync)>,
    ) -> Result<FetchResult> {
        use tracing::warn;
        use crate::protocol::{BalanceResult, TransactionResult};
        use crate::types::{Mt940Data, TransactionStatus, TouchdownPoint};

        // ── Balance ──
        let balance = if opts.balance {
            match dialog.balance(account).await {
                Ok(BalanceResult::Success(b)) => Some(b),
                Ok(BalanceResult::NeedTan(challenge)) => {
                    if let Some(notify) = on_tan {
                        notify(true);
                        await_decoupled_tan(dialog, &challenge.task_reference).await?;
                        notify(false);
                        match dialog.balance(account).await {
                            Ok(BalanceResult::Success(b)) => Some(b),
                            Ok(_) => { warn!("Balance still unavailable after TAN"); None }
                            Err(e) => { warn!("Balance retry failed: {}", e); None }
                        }
                    } else {
                        warn!("Balance requires TAN — skipping");
                        None
                    }
                }
                Ok(BalanceResult::Empty) => None,
                Err(e) => { warn!("Balance failed: {}", e); None }
            }
        } else {
            None
        };

        // ── Transactions ──
        let mut transactions = if opts.transactions {
            let end_date = chrono::Utc::now().date_naive();
            let start_date = end_date - chrono::Duration::days(opts.days.max(1) as i64);
            let mut all_booked = Mt940Data::new();
            let mut all_pending = Mt940Data::new();
            let mut td: Option<TouchdownPoint> = None;
            loop {
                match dialog.transactions(account, start_date, end_date, td.as_ref()).await? {
                    TransactionResult::NeedTan(challenge) => {
                        if let Some(notify) = on_tan {
                            notify(true);
                            await_decoupled_tan(dialog, &challenge.task_reference).await?;
                            notify(false);
                            continue; // retry same page
                        }
                        break;
                    }
                    TransactionResult::Success(page) => {
                        if !page.booked.is_empty() { all_booked.extend(page.booked.0); }
                        if !page.pending.is_empty() { all_pending.extend(page.pending.0); }
                        td = page.touchdown;
                        if td.is_none() { break; }
                    }
                }
            }
            let mut txns = match parse_mt940(all_booked.as_bytes(), TransactionStatus::Booked) {
                Ok(t) => t,
                Err(e) => {
                    warn!("MT940 booked parse failed ({} bytes): {}", all_booked.as_bytes().len(), e);
                    Vec::new()
                }
            };
            if !all_pending.is_empty() {
                match parse_mt940(all_pending.as_bytes(), TransactionStatus::Pending) {
                    Ok(t) => txns.extend(t),
                    Err(e) => warn!("MT940 pending parse failed ({} bytes): {}", all_pending.as_bytes().len(), e),
                }
            }
            txns
        } else {
            Vec::new()
        };

        // ── Holdings ──
        let holdings = if opts.holdings {
            match self.fetch_holdings(dialog, account).await {
                Ok(h) => h,
                Err(e) => { warn!("Holdings fetch failed: {}", e); Vec::new() }
            }
        } else {
            Vec::new()
        };

        // ── Credit card transactions (DKKKU) ──
        if opts.credit_card {
            if let Some(ref card_number) = opts.credit_card_number {
                let start_date = if opts.days > 0 {
                    Some(chrono::Utc::now().date_naive() - chrono::Duration::days(opts.days.max(1) as i64))
                } else {
                    None
                };
                let mut td: Option<TouchdownPoint> = None;
                loop {
                    use crate::protocol::CreditCardTransactionResult;
                    match dialog.credit_card_transactions(account, card_number, start_date, td.as_ref()).await {
                        Ok(CreditCardTransactionResult::Success(page)) => {
                            info!("[FinTS] DKKKU: {} credit card transactions", page.transactions.len());
                            transactions.extend(page.transactions);
                            td = page.touchdown;
                            if td.is_none() { break; }
                        }
                        Ok(CreditCardTransactionResult::NeedTan(_)) => {
                            warn!("Credit card transactions require TAN — skipping");
                            break;
                        }
                        Err(e) => {
                            warn!("Credit card transactions failed: {}", e);
                            break;
                        }
                    }
                }
            }
        }

        Ok(FetchResult { balance, transactions, holdings })
    }
}

const TAN_POLL_MAX: u32 = 30;
const TAN_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3);

async fn await_decoupled_tan(
    dialog: &mut Dialog<Open>,
    task_reference: &crate::types::TaskReference,
) -> Result<()> {
    use tracing::info;
    for attempt in 1..=TAN_POLL_MAX {
        tokio::time::sleep(TAN_POLL_INTERVAL).await;
        if dialog.poll_decoupled_tan(task_reference).await? {
            info!("[FinTS] Per-request TAN confirmed after {attempt} polls");
            return Ok(());
        }
    }
    Err(crate::FinTSError::Dialog(
        "pushTAN was not confirmed in time for the data request".into(),
    ))
}

/// Look up a bank implementation by its BLZ (Bankleitzahl).
///
/// The BLZ is the canonical bank identifier — banks are dispatched based on it.
/// BLZ `12030000` → DKB implementation; all others → GenericBank.
pub fn bank_ops(blz: &str) -> Result<AnyBank> {
    let config = crate::banks::bank_by_blz(blz)
        .ok_or_else(|| FinTSError::Dialog(format!("Unknown BLZ: {}", blz)))?;
    match blz {
        "12030000" => Ok(AnyBank::Dkb(Dkb::new())),
        _ => Ok(AnyBank::Generic(GenericBank::new(config))),
    }
}

/// Create a bank implementation from a custom BankConfig (for non-registry banks).
pub fn bank_ops_with_config(config: BankConfig) -> AnyBank {
    AnyBank::Generic(GenericBank::new(config))
}

// ═══════════════════════════════════════════════════════════════════════════════
// MT940 parsing
// ═══════════════════════════════════════════════════════════════════════════════

fn rewrap_tag86(content: &str, out: &mut Vec<String>) {
    let mut lines_emitted = 0u32;
    let mut pos = 0;
    while pos < content.len() && lines_emitted < 6 {
        let end = (pos + 65).min(content.len());
        let chunk = &content[pos..end];
        if lines_emitted == 0 {
            out.push(format!(":86:{chunk}"));
        } else {
            out.push(chunk.to_string());
        }
        lines_emitted += 1;
        pos = end;
    }
}

fn parse_mt940(data: &[u8], status: TransactionStatus) -> Result<Vec<Transaction>> {
    if data.is_empty() { return Ok(Vec::new()); }

    let (cow, _, had_errors) = encoding_rs::WINDOWS_1252.decode(data);
    if had_errors { warn!("MT940 encoding errors"); }
    let mt940_text = cow.into_owned();

    let filtered: Vec<&str> = mt940_text.lines()
        .filter(|l| { let t = l.trim(); !t.is_empty() && t != "-" && t != "--" })
        .collect();

    // The mt940 crate's Pest grammar limits :86: fields to 6 lines of
    // 65 chars each. German banks using DFÜ ?XX subfields sometimes
    // exceed this (7+ lines when ?60 ABWA is present). Collect each
    // :86: field's full text and re-wrap to fit the grammar.
    let mut rewrapped: Vec<String> = Vec::with_capacity(filtered.len());
    let mut tag86_buf: Option<String> = None;
    for line in &filtered {
        let is_tag = line.starts_with(':') && line.len() > 3 && line[1..].contains(':');
        if is_tag {
            if let Some(buf) = tag86_buf.take() {
                rewrap_tag86(&buf, &mut rewrapped);
            }
            if line.starts_with(":86:") {
                tag86_buf = Some(line[4..].to_string());
            } else {
                rewrapped.push(line.to_string());
            }
        } else if let Some(ref mut buf) = tag86_buf {
            buf.push_str(line);
        } else {
            rewrapped.push(line.to_string());
        }
    }
    if let Some(buf) = tag86_buf.take() {
        rewrap_tag86(&buf, &mut rewrapped);
    }
    let cleaned = rewrapped.join("\r\n") + "\r\n";

    info!("[MT940] input: {} bytes, decoded: {} chars, cleaned: {} chars",
        data.len(), mt940_text.len(), cleaned.len());
    if cleaned.len() < 200 {
        info!("[MT940] cleaned text: {:?}", &cleaned);
    } else {
        info!("[MT940] first 200 chars: {:?}", &cleaned[..200]);
    }

    if let Ok(dump_dir) = std::env::var("FINTS_DUMP_DIR") {
        let path = format!("{}/mt940_{:?}_{}.txt", dump_dir, status, data.len());
        if let Err(e) = std::fs::write(&path, &cleaned) {
            warn!("[MT940] failed to dump to {}: {}", path, e);
        } else {
            info!("[MT940] dumped cleaned text to {}", path);
        }
    }

    let sanitized = mt940::sanitizers::to_swift_charset(&cleaned);
    let messages = mt940::parse_mt940(&sanitized)
        .map_err(|e| FinTSError::Mt940(format!("MT940 parse error: {}", e)))?;
    info!("[MT940] parsed {} messages, {} total statement lines",
        messages.len(),
        messages.iter().map(|m| m.statement_lines.len()).sum::<usize>());

    let mut transactions = Vec::new();
    for msg in messages {
        for line in msg.statement_lines {
            let is_debit = matches!(line.ext_debit_credit_indicator, mt940::ExtDebitOrCredit::Debit);
            let amount = if is_debit { -line.amount } else { line.amount };

            let (applicant_name, applicant_iban, applicant_bic, purpose, posting_text) =
                match &line.information_to_account_owner {
                    Some(mt940::InformationToAccountOwner::Structured {
                        applicant_name, applicant_iban, applicant_bin, purpose, posting_text, ..
                    }) => (applicant_name.clone(), applicant_iban.clone(), applicant_bin.clone(), purpose.clone(), posting_text.clone()),
                    Some(mt940::InformationToAccountOwner::Plain(text)) => (None, None, None, Some(text.clone()), None),
                    None => (None, None, None, None, None),
                };

            let raw = serde_json::json!({
                "date": line.value_date.to_string(),
                "entry_date": line.entry_date.map(|d| d.to_string()),
                "amount": amount.to_string(),
                "currency": msg.opening_balance.iso_currency_code,
                "customer_ref": line.customer_ref,
                "bank_ref": line.bank_ref,
                "applicant_name": applicant_name,
                "applicant_iban": applicant_iban,
                "applicant_bic": applicant_bic,
                "purpose": purpose,
                "posting_text": posting_text,
            });

            transactions.push(Transaction {
                date: line.value_date, valuta_date: line.entry_date,
                amount,
                currency: Currency::new(&msg.opening_balance.iso_currency_code),
                applicant_name,
                applicant_iban: applicant_iban.map(|s| Iban::new(s)),
                applicant_bic: applicant_bic.map(|s| Bic::new(s)),
                purpose, posting_text,
                reference: Some(line.customer_ref.clone()),
                raw, status: status.clone(),
            });
        }
    }
    Ok(transactions)
}
