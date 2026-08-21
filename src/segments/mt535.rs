//! MT535 (Statement of Holdings) mini-parser.
//!
//! HIWPD carries the depot statement as a binary blob in S.W.I.F.T. MT535
//! format (analogous to HIKAZ carrying MT940 for transactions). This parses
//! the subset of fields German banks emit per position, mirroring the
//! approach of python-fints' `MT535_Miniparser`:
//!
//! ```text
//! :16R:FIN                                  start of a position block
//! :35B:ISIN LU0635178014 /DE/ETF127 NAME    identification (may span lines)
//! :90B::MRKT//ACTU/EUR30,463                actual market price
//! :90A::MRKT//PRCT/97,5                     percentage price (bonds)
//! :98A::PRIC//20170428                      price date
//! :93B::AGGR//UNIT/16,8211                  quantity (units)
//! :19A::HOLD//EUR512,43                     total market value
//! :70E::HOLD//1STK+...+DE+20160402+          German structured details
//! 250,39+EUR+                                acquisition price details
//! :16S:FIN                                  end of the position block
//! ```

use chrono::NaiveDate;
use rust_decimal::Decimal;

use crate::types::{Currency, Isin, SecurityHolding, Wkn};

/// Parse an MT535 depot statement blob into securities holdings.
pub(crate) fn parse_mt535(data: &[u8]) -> Vec<SecurityHolding> {
    let text = String::from_utf8_lossy(data);
    let lines = collapse_multilines(&text);

    let mut holdings = Vec::new();
    let mut current: Option<PositionFields> = None;

    for line in &lines {
        if line.starts_with(":16R:FIN") {
            current = Some(PositionFields::default());
            continue;
        }
        if line.starts_with(":16S:FIN") {
            if let Some(fields) = current.take() {
                if let Some(h) = fields.into_holding() {
                    holdings.push(h);
                }
            }
            continue;
        }
        let Some(fields) = current.as_mut() else { continue };

        if let Some(rest) = line.strip_prefix(":35B:") {
            fields.parse_identification(rest);
        } else if let Some(rest) = line.strip_prefix(":90B::MRKT//ACTU/") {
            // e.g. "EUR30,463"
            if rest.len() > 3 {
                fields.price_currency = Some(rest[..3].to_string());
                fields.price = parse_decimal(&rest[3..]);
            }
        } else if let Some(rest) = line.strip_prefix(":90A::MRKT//PRCT/") {
            // Percentage price (bonds) — no currency
            fields.price = parse_decimal(rest);
        } else if let Some(rest) = line.strip_prefix(":98A::PRIC//") {
            fields.price_date = NaiveDate::parse_from_str(rest.trim(), "%Y%m%d").ok();
        } else if let Some(rest) = line.strip_prefix(":98C::PRIC//") {
            let trimmed = rest.trim();
            if trimmed.len() >= 8 {
                fields.price_date = NaiveDate::parse_from_str(&trimmed[..8], "%Y%m%d").ok();
            }
        } else if let Some(rest) = line.strip_prefix(":93B::AGGR//UNIT/") {
            fields.quantity = parse_decimal(rest);
        } else if let Some(rest) = line.strip_prefix(":93B::AGGR//FAMT/") {
            // Face amount (bonds) — treat as quantity
            fields.quantity = parse_decimal(rest);
        } else if let Some(rest) = line.strip_prefix(":19A::HOLD//") {
            if rest.len() > 3 {
                fields.market_value_currency = Some(rest[..3].to_string());
                fields.market_value = parse_decimal(&rest[3..]);
            }
        } else if let Some(rest) = line.strip_prefix(":70E::HOLD//") {
            fields.parse_holdings_narrative(rest);
        }
    }

    holdings
}

#[derive(Default)]
struct PositionFields {
    isin: Option<String>,
    wkn: Option<String>,
    name: String,
    quantity: Option<Decimal>,
    price: Option<Decimal>,
    price_currency: Option<String>,
    price_date: Option<NaiveDate>,
    acquisition_date: Option<NaiveDate>,
    acquisition_price: Option<Decimal>,
    acquisition_price_currency: Option<String>,
    market_value: Option<Decimal>,
    market_value_currency: Option<String>,
}

impl PositionFields {
    /// Parse the :35B: identification, e.g. after multiline collapse:
    /// `ISIN LU0635178014 /DE/ETF127|COMST.-MSCI EM.MKTS.TRN U.ETF I`
    /// The ISIN follows the "ISIN " keyword; the German WKN is encoded as
    /// "/DE/<wkn>"; everything else is the security name.
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
                // German banks encode the WKN as "/DE/<wkn>"
                self.wkn = Some(wkn.to_string());
            } else {
                name_parts.push(part);
            }
        }
        self.name = name_parts.join(" ");
    }

    /// Parse the German structured form of MT535 `70E::HOLD`.
    ///
    /// Each physical line begins with its line number and the remaining
    /// subfields are `+`-separated. On line 1, subfield 6 is the acquisition
    /// date. On line 2, subfields 9 and 10 are the acquisition price and its
    /// optional currency. After multiline collapse, for example:
    /// `1STK+812+00028+DE+20260402+|250,39+EUR+|3|4`.
    fn parse_holdings_narrative(&mut self, rest: &str) {
        for line in rest.split('|') {
            if let Some(line_one) = line.strip_prefix('1') {
                let fields: Vec<_> = line_one.split('+').collect();
                if let Some(value) = fields.get(4).map(|value| value.trim()) {
                    if !value.is_empty() {
                        self.acquisition_date =
                            NaiveDate::parse_from_str(value, "%Y%m%d").ok();
                    }
                }
            } else if let Some(line_two) = line.strip_prefix('2') {
                let fields: Vec<_> = line_two.split('+').collect();
                if let Some(value) = fields.first().map(|value| value.trim()) {
                    if !value.is_empty() {
                        self.acquisition_price = parse_decimal(value);
                    }
                }
                if let Some(value) = fields.get(1).map(|value| value.trim()) {
                    if !value.is_empty() {
                        self.acquisition_price_currency = Some(value.to_string());
                    }
                }
            }
        }
    }

    fn into_holding(self) -> Option<SecurityHolding> {
        // A position needs at least an identifier or a name
        if self.isin.is_none() && self.wkn.is_none() && self.name.is_empty() {
            return None;
        }
        Some(SecurityHolding {
            isin: self.isin.map(Isin::new),
            wkn: self.wkn.map(Wkn::new),
            name: self.name,
            quantity: self.quantity.unwrap_or_default(),
            price: self.price,
            price_currency: self.price_currency.map(Currency::new),
            price_date: self.price_date,
            acquisition_date: self.acquisition_date,
            acquisition_price: self.acquisition_price,
            acquisition_price_currency: self.acquisition_price_currency.map(Currency::new),
            market_value: self.market_value,
            market_value_currency: self.market_value_currency.map(Currency::new),
            acquisition_value: None,
            profit_loss: None,
            exchange: None,
            depot_id: None,
            raw: serde_json::Value::Null,
        })
    }
}

/// Join continuation lines (not starting with ':' or '-') onto the previous
/// field line with a '|' separator, so multi-line fields like :35B: become a
/// single logical line.
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

/// Parse a SWIFT decimal (comma as decimal separator), e.g. "16,8211".
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
:28E:1/ONLY\r\n\
:13A::STAT//086\r\n\
:16S:GENL\r\n\
:16R:FIN\r\n\
:35B:ISIN LU0635178014 /DE/ETF127\r\n\
COMST.-MSCI EM.MKTS.TRN U.ETF\r\n\
:93B::AGGR//UNIT/16,8211\r\n\
:16R:SUBBAL\r\n\
:93C::TAVI//UNIT/16,8211/AVAI\r\n\
:16S:SUBBAL\r\n\
:19A::HOLD//EUR512,43\r\n\
:70E::HOLD//1STK23,968662\r\n\
:90B::MRKT//ACTU/EUR30,463\r\n\
:98A::PRIC//20170428\r\n\
:16S:FIN\r\n\
:16R:ADDINFO\r\n\
:19A::HOLP//EUR512,43\r\n\
:16S:ADDINFO\r\n";

    #[test]
    fn parses_single_position() {
        let holdings = parse_mt535(SAMPLE.as_bytes());
        assert_eq!(holdings.len(), 1);
        let h = &holdings[0];
        assert_eq!(h.isin.as_ref().map(|i| i.as_str()), Some("LU0635178014"));
        assert_eq!(h.wkn.as_ref().map(|w| w.as_str()), Some("ETF127"));
        assert_eq!(h.name, "COMST.-MSCI EM.MKTS.TRN U.ETF");
        assert_eq!(h.quantity, dec("16.8211"));
        assert_eq!(h.price, Some(dec("30.463")));
        assert_eq!(h.price_currency.as_ref().map(|c| c.as_str()), Some("EUR"));
        assert_eq!(
            h.price_date,
            NaiveDate::from_ymd_opt(2017, 4, 28)
        );
        assert_eq!(h.market_value, Some(dec("512.43")));
        assert_eq!(
            h.market_value_currency.as_ref().map(|c| c.as_str()),
            Some("EUR")
        );
    }

    #[test]
    fn parses_multiple_positions() {
        let two = format!(
            "{}:16R:FIN\r\n:35B:ISIN IE00B4L5Y983\r\niShares Core MSCI World\r\n:93B::AGGR//UNIT/100,0\r\n:19A::HOLD//EUR8500,00\r\n:16S:FIN\r\n",
            SAMPLE
        );
        let holdings = parse_mt535(two.as_bytes());
        assert_eq!(holdings.len(), 2);
        assert_eq!(
            holdings[1].isin.as_ref().map(|i| i.as_str()),
            Some("IE00B4L5Y983")
        );
        assert_eq!(holdings[1].quantity, dec("100.0"));
    }

    #[test]
    fn empty_blob_yields_nothing() {
        assert!(parse_mt535(b"").is_empty());
        assert!(parse_mt535(b":16R:GENL\r\n:16S:GENL\r\n").is_empty());
    }

    #[test]
    fn percentage_price_bond() {
        let bond = "\
:16R:FIN\r\n\
:35B:ISIN DE0001102341\r\nBUNDANL.V.14/24\r\n\
:93B::AGGR//FAMT/5000,\r\n\
:90A::MRKT//PRCT/97,42\r\n\
:19A::HOLD//EUR4871,00\r\n\
:16S:FIN\r\n";
        let holdings = parse_mt535(bond.as_bytes());
        assert_eq!(holdings.len(), 1);
        assert_eq!(holdings[0].quantity, dec("5000"));
        assert_eq!(holdings[0].price, Some(dec("97.42")));
        assert!(holdings[0].price_currency.is_none());
    }

    #[test]
    fn parses_german_structured_acquisition_details() {
        let statement = "\
:16R:FIN\r\n\
:35B:ISIN DE000A41ED69 /DE/A41ED6\r\n\
TBF SMART POWER INHABER-ANTEILE EUR S\r\n\
:93B::AGGR//UNIT/589,78\r\n\
:70E::HOLD//1STK+812+00028+DE+20260402+\r\n\
250,39+EUR+\r\n\
3\r\n\
4\r\n\
:16S:FIN\r\n";

        let holdings = parse_mt535(statement.as_bytes());
        assert_eq!(holdings.len(), 1);
        assert_eq!(
            holdings[0].acquisition_date,
            NaiveDate::from_ymd_opt(2026, 4, 2)
        );
        assert_eq!(holdings[0].acquisition_price, Some(dec("50.39")));
        assert_eq!(
            holdings[0]
                .acquisition_price_currency
                .as_ref()
                .map(|currency| currency.as_str()),
            Some("EUR")
        );
    }
}
