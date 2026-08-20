//! MT536 (Statement of Transactions) mini-parser.
//!
//! HIWDU carries depot transactions as a binary blob in S.W.I.F.T. MT536
//! format (analogous to HIWPD carrying MT535 for holdings). This parses
//! the subset of fields German banks emit per transaction:
//!
//! ```text
//! :16R:GENL                         general info (skipped)
//! :16S:GENL
//! :16R:FIN                          per-security block
//! :35B:ISIN LU0635178014 ...        security identification
//! :16R:TRAN                         per-transaction block
//!   :16R:TRANSDET                   transaction details
//!     :36B::PSTA//UNIT/16,8211      quantity
//!     :19A::PSTA//EUR512,43         posted amount
//!     :22H::REDE//RECE             direction (RECE=buy, DELI=sell)
//!     :22F::TRAN//SETT             type (SETT=settlement, CORP=corporate action)
//!     :98A::ESET//20170428          effective date
//!     :98A::SETT//20170502          settlement date
//!     :25D::MOVE//REVE             reversal flag (optional)
//!     :70E::TRDE//details          free text
//!   :16S:TRANSDET
//! :16S:TRAN
//! :16S:FIN
//! ```

use chrono::NaiveDate;
use rust_decimal::Decimal;

use crate::types::{Currency, DepotTransaction, DepotTransactionDirection, Isin, Wkn};

/// Parse an MT536 statement of transactions blob.
pub(crate) fn parse_mt536(data: &[u8]) -> Vec<DepotTransaction> {
    let text = String::from_utf8_lossy(data);
    let lines = collapse_multilines(&text);

    let mut transactions = Vec::new();
    let mut current_security: Option<SecurityIdent> = None;
    let mut current_txn: Option<TxnFields> = None;

    for line in &lines {
        // Security block boundaries
        if line.starts_with(":16R:FIN") {
            current_security = Some(SecurityIdent::default());
            continue;
        }
        if line.starts_with(":16S:FIN") {
            current_security = None;
            continue;
        }

        // Transaction block boundaries
        if line.starts_with(":16R:TRAN") {
            if let Some(sec) = &current_security {
                current_txn = Some(TxnFields {
                    isin: sec.isin.clone(),
                    wkn: sec.wkn.clone(),
                    name: sec.name.clone(),
                    ..Default::default()
                });
            }
            continue;
        }
        if line.starts_with(":16S:TRAN") {
            if let Some(txn) = current_txn.take() {
                if let Some(t) = txn.into_transaction() {
                    transactions.push(t);
                }
            }
            continue;
        }

        // Skip TRANSDET block markers (they nest inside TRAN)
        if line.starts_with(":16R:TRANSDET") || line.starts_with(":16S:TRANSDET") {
            continue;
        }

        // Security identification (inside FIN, outside TRAN)
        if let Some(rest) = line.strip_prefix(":35B:") {
            if current_txn.is_none() {
                if let Some(sec) = current_security.as_mut() {
                    sec.parse_identification(rest);
                }
            }
            continue;
        }

        // Transaction detail fields (inside TRAN)
        let Some(txn) = current_txn.as_mut() else { continue };

        if let Some(rest) = line.strip_prefix(":36B::PSTA//UNIT/") {
            txn.quantity = parse_decimal(rest);
        } else if let Some(rest) = line.strip_prefix(":36B::PSTA//FAMT/") {
            txn.quantity = parse_decimal(rest);
        } else if let Some(rest) = line.strip_prefix(":19A::PSTA//") {
            if rest.len() > 3 {
                txn.currency = Some(rest[..3].to_string());
                txn.amount = parse_decimal(&rest[3..]);
            }
        } else if let Some(rest) = line.strip_prefix(":22H::REDE//") {
            txn.direction = match rest.trim() {
                "RECE" => Some(DepotTransactionDirection::Receive),
                "DELI" => Some(DepotTransactionDirection::Deliver),
                _ => Some(DepotTransactionDirection::Unknown),
            };
        } else if let Some(rest) = line.strip_prefix(":98A::ESET//") {
            txn.effective_date = NaiveDate::parse_from_str(rest.trim(), "%Y%m%d").ok();
        } else if let Some(rest) = line.strip_prefix(":98A::SETT//") {
            txn.settlement_date = NaiveDate::parse_from_str(rest.trim(), "%Y%m%d").ok();
        } else if let Some(rest) = line.strip_prefix(":25D::MOVE//") {
            if rest.trim() == "REVE" {
                txn.is_reversal = true;
            }
        } else if let Some(rest) = line.strip_prefix(":70E::TRDE//") {
            txn.details = Some(rest.replace('|', "\n"));
        }
    }

    transactions
}

#[derive(Default)]
struct SecurityIdent {
    isin: Option<String>,
    wkn: Option<String>,
    name: String,
}

impl SecurityIdent {
    fn parse_identification(&mut self, rest: &str) {
        let mut name_parts: Vec<&str> = Vec::new();
        let mut expect_isin = false;
        for part in rest.split(['|', ' ']) {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            if part == "ISIN" {
                expect_isin = true;
            } else if expect_isin {
                self.isin = Some(part.to_string());
                expect_isin = false;
            } else if let Some(wkn) = part.strip_prefix("/DE/") {
                self.wkn = Some(wkn.to_string());
            } else {
                name_parts.push(part);
            }
        }
        self.name = name_parts.join(" ");
    }
}

#[derive(Default)]
struct TxnFields {
    isin: Option<String>,
    wkn: Option<String>,
    name: String,
    quantity: Option<Decimal>,
    amount: Option<Decimal>,
    currency: Option<String>,
    direction: Option<DepotTransactionDirection>,
    effective_date: Option<NaiveDate>,
    settlement_date: Option<NaiveDate>,
    is_reversal: bool,
    details: Option<String>,
}

impl TxnFields {
    fn into_transaction(self) -> Option<DepotTransaction> {
        // Need at least a direction or a date to be meaningful
        if self.direction.is_none() && self.effective_date.is_none() && self.settlement_date.is_none() {
            return None;
        }
        Some(DepotTransaction {
            isin: self.isin.map(Isin::new),
            wkn: self.wkn.map(Wkn::new),
            name: self.name,
            quantity: self.quantity.unwrap_or_default(),
            amount: self.amount,
            currency: self.currency.map(Currency::new),
            direction: self.direction.unwrap_or(DepotTransactionDirection::Unknown),
            effective_date: self.effective_date,
            settlement_date: self.settlement_date,
            is_reversal: self.is_reversal,
            details: self.details,
        })
    }
}

fn collapse_multilines(text: &str) -> Vec<String> {
    let mut collapsed: Vec<String> = Vec::new();
    for raw in text.split(['\r', '\n']) {
        let line = raw.trim_end();
        if line.is_empty() {
            continue;
        }
        if line.starts_with(':') || line.starts_with('-') {
            collapsed.push(line.to_string());
        } else if let Some(prev) = collapsed.last_mut() {
            prev.push('|');
            prev.push_str(line);
        }
    }
    collapsed
}

fn parse_decimal(s: &str) -> Option<Decimal> {
    let normalized = s.trim().replace(',', ".");
    normalized.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dec(s: &str) -> Decimal {
        s.parse().expect("valid decimal literal")
    }

    const SAMPLE: &str = "\
:16R:GENL\r\n\
:98C::PREP//20170502120000\r\n\
:97A::SAFE//20050550/8030189693\r\n\
:17B::ACTI//Y\r\n\
:16S:GENL\r\n\
:16R:FIN\r\n\
:35B:ISIN LU0635178014 /DE/ETF127\r\n\
COMST.-MSCI EM.MKTS.TRN U.ETF\r\n\
:16R:TRAN\r\n\
:16R:TRANSDET\r\n\
:36B::PSTA//UNIT/16,8211\r\n\
:19A::PSTA//EUR512,43\r\n\
:22H::REDE//RECE\r\n\
:22F::TRAN//SETT\r\n\
:98A::ESET//20170328\r\n\
:98A::SETT//20170330\r\n\
:70E::TRDE//KAUF XETRA\r\n\
:16S:TRANSDET\r\n\
:16S:TRAN\r\n\
:16S:FIN\r\n";

    #[test]
    fn parses_single_buy() {
        let txns = parse_mt536(SAMPLE.as_bytes());
        assert_eq!(txns.len(), 1);
        let t = &txns[0];
        assert_eq!(t.isin.as_ref().map(|i| i.as_str()), Some("LU0635178014"));
        assert_eq!(t.wkn.as_ref().map(|w| w.as_str()), Some("ETF127"));
        assert_eq!(t.name, "COMST.-MSCI EM.MKTS.TRN U.ETF");
        assert_eq!(t.quantity, dec("16.8211"));
        assert_eq!(t.amount, Some(dec("512.43")));
        assert_eq!(t.currency.as_ref().map(|c| c.as_str()), Some("EUR"));
        assert_eq!(t.direction, DepotTransactionDirection::Receive);
        assert_eq!(t.effective_date, NaiveDate::from_ymd_opt(2017, 3, 28));
        assert_eq!(t.settlement_date, NaiveDate::from_ymd_opt(2017, 3, 30));
        assert!(!t.is_reversal);
        assert_eq!(t.details.as_deref(), Some("KAUF XETRA"));
    }

    #[test]
    fn parses_sell_and_reversal() {
        let data = "\
:16R:FIN\r\n\
:35B:ISIN DE0005140008\r\n\
DEUTSCHE BANK AG\r\n\
:16R:TRAN\r\n\
:16R:TRANSDET\r\n\
:36B::PSTA//UNIT/50,\r\n\
:19A::PSTA//EUR3500,00\r\n\
:22H::REDE//DELI\r\n\
:98A::ESET//20170410\r\n\
:16S:TRANSDET\r\n\
:16S:TRAN\r\n\
:16R:TRAN\r\n\
:16R:TRANSDET\r\n\
:36B::PSTA//UNIT/10,\r\n\
:19A::PSTA//EUR700,00\r\n\
:22H::REDE//DELI\r\n\
:98A::ESET//20170415\r\n\
:25D::MOVE//REVE\r\n\
:16S:TRANSDET\r\n\
:16S:TRAN\r\n\
:16S:FIN\r\n";
        let txns = parse_mt536(data.as_bytes());
        assert_eq!(txns.len(), 2);

        assert_eq!(txns[0].direction, DepotTransactionDirection::Deliver);
        assert_eq!(txns[0].quantity, dec("50"));
        assert!(!txns[0].is_reversal);

        assert_eq!(txns[1].direction, DepotTransactionDirection::Deliver);
        assert_eq!(txns[1].quantity, dec("10"));
        assert!(txns[1].is_reversal);
    }

    #[test]
    fn multiple_securities() {
        let data = "\
:16R:FIN\r\n\
:35B:ISIN LU0635178014\r\n\
FUND A\r\n\
:16R:TRAN\r\n\
:16R:TRANSDET\r\n\
:36B::PSTA//UNIT/10,\r\n\
:22H::REDE//RECE\r\n\
:98A::ESET//20170101\r\n\
:16S:TRANSDET\r\n\
:16S:TRAN\r\n\
:16S:FIN\r\n\
:16R:FIN\r\n\
:35B:ISIN IE00B4L5Y983\r\n\
FUND B\r\n\
:16R:TRAN\r\n\
:16R:TRANSDET\r\n\
:36B::PSTA//UNIT/20,\r\n\
:22H::REDE//DELI\r\n\
:98A::ESET//20170201\r\n\
:16S:TRANSDET\r\n\
:16S:TRAN\r\n\
:16S:FIN\r\n";
        let txns = parse_mt536(data.as_bytes());
        assert_eq!(txns.len(), 2);
        assert_eq!(txns[0].isin.as_ref().map(|i| i.as_str()), Some("LU0635178014"));
        assert_eq!(txns[0].direction, DepotTransactionDirection::Receive);
        assert_eq!(txns[1].isin.as_ref().map(|i| i.as_str()), Some("IE00B4L5Y983"));
        assert_eq!(txns[1].direction, DepotTransactionDirection::Deliver);
    }

    #[test]
    fn empty_blob_yields_nothing() {
        assert!(parse_mt536(b"").is_empty());
        assert!(parse_mt536(b":16R:GENL\r\n:16S:GENL\r\n").is_empty());
    }

    #[test]
    fn face_amount_bonds() {
        let data = "\
:16R:FIN\r\n\
:35B:ISIN DE0001102341\r\n\
BUNDESANL.V.14/24\r\n\
:16R:TRAN\r\n\
:16R:TRANSDET\r\n\
:36B::PSTA//FAMT/5000,\r\n\
:19A::PSTA//EUR4871,00\r\n\
:22H::REDE//RECE\r\n\
:98A::ESET//20170315\r\n\
:16S:TRANSDET\r\n\
:16S:TRAN\r\n\
:16S:FIN\r\n";
        let txns = parse_mt536(data.as_bytes());
        assert_eq!(txns.len(), 1);
        assert_eq!(txns[0].quantity, dec("5000"));
        assert_eq!(txns[0].amount, Some(dec("4871")));
    }
}
