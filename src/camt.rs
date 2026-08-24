use chrono::NaiveDate;
use quick_xml::events::Event;
use quick_xml::Reader;
use rust_decimal::Decimal;
use std::str::FromStr;

use crate::types::{Bic, Currency, Iban, Transaction, TransactionStatus};

pub fn parse_camt052(xml: &[u8]) -> Result<Vec<Transaction>, String> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    let mut path: Vec<String> = Vec::new();
    let mut report_currency: Option<String> = None;
    let mut transactions = Vec::new();
    let mut entry: Option<EntryBuilder> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let local = local_name(e.name().as_ref());
                path.push(local.clone());

                if local == "Ntry" && path_ends_with(&path, &["Rpt", "Ntry"]) {
                    entry = Some(EntryBuilder::new());
                }

                if local == "Amt" {
                    if let Some(ref mut eb) = entry {
                        for attr in e.attributes().flatten() {
                            if local_name(attr.key.as_ref()) == "Ccy" {
                                eb.amount_currency =
                                    Some(String::from_utf8_lossy(&attr.value).to_string());
                            }
                        }
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                let local = local_name(e.name().as_ref());

                if local == "Ntry" {
                    if let Some(eb) = entry.take() {
                        let fallback_ccy = report_currency.as_deref().unwrap_or("EUR");
                        match eb.build(fallback_ccy) {
                            Ok(tx) => transactions.push(tx),
                            Err(err) => return Err(err),
                        }
                    }
                }

                if path.last().map(|s| s.as_str()) == Some(&local) {
                    path.pop();
                }
            }
            Ok(Event::Text(ref e)) => {
                let text = e.unescape().map_err(|err| err.to_string())?.to_string();
                if text.is_empty() {
                    continue;
                }

                if path_ends_with(&path, &["Rpt", "Acct", "Ccy"]) && entry.is_none() {
                    report_currency = Some(text.clone());
                }

                if let Some(ref mut eb) = entry {
                    handle_text(eb, &path, &text);
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(err) => return Err(format!("XML parse error: {err}")),
        }
        buf.clear();
    }

    Ok(transactions)
}

fn local_name(name: &[u8]) -> String {
    let full = std::str::from_utf8(name).unwrap_or("");
    full.rsplit_once(':').map_or(full, |(_, local)| local).to_string()
}

fn path_ends_with(path: &[String], suffix: &[&str]) -> bool {
    if path.len() < suffix.len() {
        return false;
    }
    let start = path.len() - suffix.len();
    path[start..].iter().zip(suffix).all(|(a, b)| a == b)
}

fn handle_text(eb: &mut EntryBuilder, path: &[String], text: &str) {
    if path_ends_with(path, &["Ntry", "Amt"]) {
        eb.amount_text = Some(text.to_string());
    } else if path_ends_with(path, &["Ntry", "CdtDbtInd"]) {
        eb.cdt_dbt_ind = Some(text.to_string());
    } else if path_ends_with(path, &["Ntry", "Sts"]) {
        eb.status = Some(text.to_string());
    } else if path_ends_with(path, &["Ntry", "BookgDt", "Dt"]) {
        eb.booking_date = Some(text.to_string());
    } else if path_ends_with(path, &["Ntry", "ValDt", "Dt"]) {
        eb.valuta_date = Some(text.to_string());
    } else if path_ends_with(path, &["Ntry", "AddtlNtryInf"]) {
        eb.addtl_ntry_inf = Some(text.to_string());
    } else if path_ends_with(path, &["TxDtls", "RmtInf", "Ustrd"]) {
        eb.remittance_info = Some(text.to_string());
    } else if path_ends_with(path, &["TxDtls", "RltdPties", "Dbtr", "Nm"]) {
        eb.debtor_name = Some(text.to_string());
    } else if path_ends_with(path, &["TxDtls", "RltdPties", "DbtrAcct", "Id", "IBAN"]) {
        eb.debtor_iban = Some(text.to_string());
    } else if path_ends_with(path, &["TxDtls", "RltdPties", "Cdtr", "Nm"]) {
        eb.creditor_name = Some(text.to_string());
    } else if path_ends_with(path, &["TxDtls", "RltdPties", "CdtrAcct", "Id", "IBAN"]) {
        eb.creditor_iban = Some(text.to_string());
    } else if path_ends_with(path, &["TxDtls", "RltdAgts", "DbtrAgt", "FinInstnId", "BIC"]) {
        eb.debtor_bic = Some(text.to_string());
    } else if path_ends_with(path, &["TxDtls", "RltdAgts", "CdtrAgt", "FinInstnId", "BIC"]) {
        eb.creditor_bic = Some(text.to_string());
    } else if path_ends_with(path, &["TxDtls", "Refs", "EndToEndId"]) {
        eb.end_to_end_id = Some(text.to_string());
    } else if path_ends_with(path, &["TxDtls", "Refs", "MndtId"]) {
        eb.mandate_id = Some(text.to_string());
    } else if path_ends_with(path, &["TxDtls", "Refs", "Prtry", "Tp"]) {
        eb.prtry_ref_type = Some(text.to_string());
    } else if path_ends_with(path, &["TxDtls", "Refs", "Prtry", "Ref"]) {
        eb.prtry_ref_value = Some(text.to_string());
    } else if path_ends_with(path, &["TxDtls", "BkTxCd", "Prtry", "Cd"]) {
        eb.bk_tx_cd = Some(text.to_string());
    } else if path_ends_with(path, &["TxDtls", "RltdPties", "UltmtDbtr", "Nm"]) {
        eb.ultimate_debtor_name = Some(text.to_string());
    } else if path_ends_with(path, &["TxDtls", "RltdPties", "UltmtCdtr", "Nm"]) {
        eb.ultimate_creditor_name = Some(text.to_string());
    } else if path_ends_with(path, &["TxDtls", "Purp", "Cd"]) {
        eb.purpose_code = Some(text.to_string());
    }
}

#[derive(Default)]
struct EntryBuilder {
    amount_text: Option<String>,
    amount_currency: Option<String>,
    cdt_dbt_ind: Option<String>,
    status: Option<String>,
    booking_date: Option<String>,
    valuta_date: Option<String>,
    addtl_ntry_inf: Option<String>,
    remittance_info: Option<String>,
    debtor_name: Option<String>,
    debtor_iban: Option<String>,
    debtor_bic: Option<String>,
    creditor_name: Option<String>,
    creditor_iban: Option<String>,
    creditor_bic: Option<String>,
    ultimate_debtor_name: Option<String>,
    ultimate_creditor_name: Option<String>,
    purpose_code: Option<String>,
    end_to_end_id: Option<String>,
    mandate_id: Option<String>,
    prtry_ref_type: Option<String>,
    prtry_ref_value: Option<String>,
    bk_tx_cd: Option<String>,
}

impl EntryBuilder {
    fn new() -> Self {
        Self::default()
    }

    fn build(self, fallback_currency: &str) -> Result<Transaction, String> {
        let booking_date_str = self
            .booking_date
            .as_deref()
            .ok_or("Ntry missing BookgDt/Dt")?;
        let date = NaiveDate::parse_from_str(booking_date_str, "%Y-%m-%d")
            .map_err(|e| format!("invalid BookgDt date '{booking_date_str}': {e}"))?;

        let valuta_date = self
            .valuta_date
            .as_deref()
            .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());

        let amount_str = self.amount_text.as_deref().ok_or("Ntry missing Amt")?;
        let mut amount = Decimal::from_str(amount_str)
            .map_err(|e| format!("invalid amount '{amount_str}': {e}"))?;

        let is_debit = self.cdt_dbt_ind.as_deref() == Some("DBIT");
        if is_debit {
            amount = -amount;
        }

        let currency_str = self
            .amount_currency
            .as_deref()
            .unwrap_or(fallback_currency);
        let currency = Currency::new(currency_str);

        // The ultimate counterparty is the actual merchant (e.g. the shop
        // behind a PayPal or card payment). Prefer it over the direct
        // counterparty, which is often just the payment processor.
        let (applicant_name, applicant_iban, applicant_bic) = if is_debit {
            (
                self.ultimate_creditor_name.or(self.creditor_name),
                self.creditor_iban.map(Iban::new),
                self.creditor_bic.map(Bic::new),
            )
        } else {
            (
                self.ultimate_debtor_name.or(self.debtor_name),
                self.debtor_iban.map(Iban::new),
                self.debtor_bic.map(Bic::new),
            )
        };

        let reference = if self.prtry_ref_type.as_deref() == Some("FI-UMSATZ-ID")
        {
            self.prtry_ref_value
        } else {
            self.end_to_end_id.clone()
        };

        let status = match self.status.as_deref() {
            Some("PDNG") => TransactionStatus::Pending,
            _ => TransactionStatus::Booked,
        };

        let raw = serde_json::json!({
            "booking_date": booking_date_str,
            "cdt_dbt_ind": self.cdt_dbt_ind.as_deref().unwrap_or(""),
            "bk_tx_cd": self.bk_tx_cd.as_deref().unwrap_or(""),
            "end_to_end_id": self.end_to_end_id.as_deref().unwrap_or(""),
            "mandate_id": self.mandate_id.as_deref().unwrap_or(""),
        });

        Ok(Transaction {
            date,
            valuta_date,
            amount,
            currency,
            applicant_name,
            applicant_iban,
            applicant_bic,
            purpose: self.remittance_info,
            posting_text: self.addtl_ntry_inf,
            reference,
            purpose_code: self.purpose_code,
            mandate_id: self.mandate_id.clone(),
            raw,
            status,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_document() -> &'static [u8] {
        br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns="urn:iso:std:iso:20022:tech:xsd:camt.052.001.02">
  <BkToCstmrAcctRpt>
    <GrpHdr><MsgId>MSG001</MsgId></GrpHdr>
    <Rpt>
      <Acct><Id><IBAN>DE89370400440532013000</IBAN></Id><Ccy>EUR</Ccy></Acct>
      <Ntry>
        <Amt Ccy="EUR">42.50</Amt>
        <CdtDbtInd>DBIT</CdtDbtInd>
        <Sts>BOOK</Sts>
        <BookgDt><Dt>2026-08-20</Dt></BookgDt>
        <ValDt><Dt>2026-08-20</Dt></ValDt>
        <AddtlNtryInf>FOLGELASTSCHRIFT</AddtlNtryInf>
        <NtryDtls>
          <TxDtls>
            <Refs>
              <EndToEndId>E2E-REF-001</EndToEndId>
              <MndtId>MNDT-001</MndtId>
              <Prtry>
                <Tp>FI-UMSATZ-ID</Tp>
                <Ref>2026-08-20-11.22.33.123456</Ref>
              </Prtry>
            </Refs>
            <BkTxCd>
              <Prtry>
                <Cd>NDDT+105+9208+888</Cd>
                <Issr>DK</Issr>
              </Prtry>
            </BkTxCd>
            <RltdPties>
              <Dbtr><Nm>Max Mustermann</Nm></Dbtr>
              <DbtrAcct><Id><IBAN>DE89370400440532013000</IBAN></Id></DbtrAcct>
              <Cdtr><Nm>Online Shop GmbH</Nm></Cdtr>
              <CdtrAcct><Id><IBAN>DE27100777770209299700</IBAN></Id></CdtrAcct>
            </RltdPties>
            <RltdAgts>
              <DbtrAgt><FinInstnId><BIC>COBADEFFXXX</BIC></FinInstnId></DbtrAgt>
              <CdtrAgt><FinInstnId><BIC>DEUTDEDBPAL</BIC></FinInstnId></CdtrAgt>
            </RltdAgts>
            <RmtInf>
              <Ustrd>Order 12345 Online Shop GmbH</Ustrd>
            </RmtInf>
          </TxDtls>
        </NtryDtls>
      </Ntry>
      <Ntry>
        <Amt Ccy="EUR">1500.00</Amt>
        <CdtDbtInd>CRDT</CdtDbtInd>
        <Sts>BOOK</Sts>
        <BookgDt><Dt>2026-08-19</Dt></BookgDt>
        <ValDt><Dt>2026-08-19</Dt></ValDt>
        <AddtlNtryInf>GUTSCHRIFT</AddtlNtryInf>
        <NtryDtls>
          <TxDtls>
            <Refs>
              <EndToEndId>SALARY-AUG-2026</EndToEndId>
            </Refs>
            <RltdPties>
              <Dbtr><Nm>Arbeitgeber AG</Nm></Dbtr>
              <DbtrAcct><Id><IBAN>DE44500105175407324931</IBAN></Id></DbtrAcct>
              <Cdtr><Nm>Max Mustermann</Nm></Cdtr>
              <CdtrAcct><Id><IBAN>DE89370400440532013000</IBAN></Id></CdtrAcct>
            </RltdPties>
            <RltdAgts>
              <DbtrAgt><FinInstnId><BIC>INGDDEFFXXX</BIC></FinInstnId></DbtrAgt>
              <CdtrAgt><FinInstnId><BIC>COBADEFFXXX</BIC></FinInstnId></CdtrAgt>
            </RltdAgts>
            <RmtInf>
              <Ustrd>Gehalt August 2026</Ustrd>
            </RmtInf>
          </TxDtls>
        </NtryDtls>
      </Ntry>
      <Ntry>
        <Amt Ccy="EUR">8.90</Amt>
        <CdtDbtInd>DBIT</CdtDbtInd>
        <Sts>BOOK</Sts>
        <BookgDt><Dt>2026-08-18</Dt></BookgDt>
        <ValDt><Dt>2026-08-18</Dt></ValDt>
        <AddtlNtryInf>KARTENZAHLUNG</AddtlNtryInf>
        <NtryDtls>
          <TxDtls>
            <Refs>
              <EndToEndId>CARD-TXN-99887</EndToEndId>
              <Prtry>
                <Tp>FI-UMSATZ-ID</Tp>
                <Ref>2026-08-18-09.15.42.789012</Ref>
              </Prtry>
            </Refs>
            <RltdPties>
              <Cdtr><Nm>Landesbank Hessen-Thuringen</Nm></Cdtr>
              <UltmtCdtr><Nm>CIRCLE K VETLANDA/STORGATAN 51</Nm></UltmtCdtr>
            </RltdPties>
            <Purp><Cd>IDCP</Cd></Purp>
            <RmtInf>
              <Ustrd>Kartenzahlung 10.50 SEK Kurs 1.1798</Ustrd>
            </RmtInf>
          </TxDtls>
        </NtryDtls>
      </Ntry>
      <Ntry>
        <Amt Ccy="EUR">25.00</Amt>
        <CdtDbtInd>DBIT</CdtDbtInd>
        <Sts>PDNG</Sts>
        <BookgDt><Dt>2026-08-21</Dt></BookgDt>
        <AddtlNtryInf>DAUERAUFTRAG</AddtlNtryInf>
        <NtryDtls>
          <TxDtls>
            <Refs>
              <EndToEndId>STANDING-ORDER-42</EndToEndId>
            </Refs>
            <RltdPties>
              <Cdtr><Nm>Verein ABC e.V.</Nm></Cdtr>
              <CdtrAcct><Id><IBAN>DE75512108001245126199</IBAN></Id></CdtrAcct>
            </RltdPties>
            <RmtInf>
              <Ustrd>Mitgliedsbeitrag</Ustrd>
            </RmtInf>
          </TxDtls>
        </NtryDtls>
      </Ntry>
      <Ntry>
        <Amt Ccy="EUR">100.00</Amt>
        <CdtDbtInd>CRDT</CdtDbtInd>
        <Sts>BOOK</Sts>
        <BookgDt><Dt>2026-08-17</Dt></BookgDt>
        <ValDt><Dt>2026-08-17</Dt></ValDt>
        <AddtlNtryInf>UEBERWEISUNG</AddtlNtryInf>
      </Ntry>
    </Rpt>
  </BkToCstmrAcctRpt>
</Document>"#
    }

    #[test]
    fn parse_debit_entry() {
        let txs = parse_camt052(test_document()).unwrap();
        let tx = &txs[0];

        assert_eq!(tx.date, NaiveDate::from_ymd_opt(2026, 8, 20).unwrap());
        assert_eq!(
            tx.valuta_date,
            Some(NaiveDate::from_ymd_opt(2026, 8, 20).unwrap())
        );
        assert_eq!(tx.amount, Decimal::from_str("-42.50").unwrap());
        assert_eq!(tx.currency.as_str(), "EUR");
        assert_eq!(tx.applicant_name.as_deref(), Some("Online Shop GmbH"));
        assert_eq!(
            tx.applicant_iban.as_ref().map(|i| i.as_str()),
            Some("DE27100777770209299700")
        );
        assert_eq!(
            tx.applicant_bic.as_ref().map(|b| b.as_str()),
            Some("DEUTDEDBPAL")
        );
        assert_eq!(tx.purpose.as_deref(), Some("Order 12345 Online Shop GmbH"));
        assert_eq!(tx.posting_text.as_deref(), Some("FOLGELASTSCHRIFT"));
        assert_eq!(
            tx.reference.as_deref(),
            Some("2026-08-20-11.22.33.123456")
        );
        assert_eq!(tx.status, TransactionStatus::Booked);
    }

    #[test]
    fn parse_credit_entry() {
        let txs = parse_camt052(test_document()).unwrap();
        let tx = &txs[1];

        assert_eq!(tx.date, NaiveDate::from_ymd_opt(2026, 8, 19).unwrap());
        assert_eq!(tx.amount, Decimal::from_str("1500.00").unwrap());
        assert_eq!(tx.applicant_name.as_deref(), Some("Arbeitgeber AG"));
        assert_eq!(
            tx.applicant_iban.as_ref().map(|i| i.as_str()),
            Some("DE44500105175407324931")
        );
        assert_eq!(
            tx.applicant_bic.as_ref().map(|b| b.as_str()),
            Some("INGDDEFFXXX")
        );
        assert_eq!(tx.purpose.as_deref(), Some("Gehalt August 2026"));
        assert_eq!(tx.posting_text.as_deref(), Some("GUTSCHRIFT"));
        assert_eq!(tx.reference.as_deref(), Some("SALARY-AUG-2026"));
        assert_eq!(tx.status, TransactionStatus::Booked);
    }

    #[test]
    fn parse_card_payment_prefers_ultimate_creditor() {
        let txs = parse_camt052(test_document()).unwrap();
        let tx = &txs[2];

        assert_eq!(tx.amount, Decimal::from_str("-8.90").unwrap());
        // UltmtCdtr is the actual merchant, preferred over the intermediary
        assert_eq!(
            tx.applicant_name.as_deref(),
            Some("CIRCLE K VETLANDA/STORGATAN 51")
        );
        assert_eq!(tx.applicant_iban, None);
        assert_eq!(tx.posting_text.as_deref(), Some("KARTENZAHLUNG"));
        assert_eq!(tx.purpose_code.as_deref(), Some("IDCP"));
        assert_eq!(
            tx.purpose.as_deref(),
            Some("Kartenzahlung 10.50 SEK Kurs 1.1798")
        );
        assert_eq!(
            tx.reference.as_deref(),
            Some("2026-08-18-09.15.42.789012")
        );
    }

    #[test]
    fn parse_pending_entry() {
        let txs = parse_camt052(test_document()).unwrap();
        let tx = &txs[3];

        assert_eq!(tx.status, TransactionStatus::Pending);
        assert_eq!(tx.amount, Decimal::from_str("-25.00").unwrap());
        assert_eq!(tx.applicant_name.as_deref(), Some("Verein ABC e.V."));
        assert_eq!(tx.valuta_date, None);
        assert_eq!(tx.reference.as_deref(), Some("STANDING-ORDER-42"));
    }

    #[test]
    fn parse_entry_without_tx_details() {
        let txs = parse_camt052(test_document()).unwrap();
        let tx = &txs[4];

        assert_eq!(tx.date, NaiveDate::from_ymd_opt(2026, 8, 17).unwrap());
        assert_eq!(tx.amount, Decimal::from_str("100.00").unwrap());
        assert_eq!(tx.applicant_name, None);
        assert_eq!(tx.applicant_iban, None);
        assert_eq!(tx.applicant_bic, None);
        assert_eq!(tx.purpose, None);
        assert_eq!(tx.posting_text.as_deref(), Some("UEBERWEISUNG"));
        assert_eq!(tx.reference, None);
    }

    #[test]
    fn parse_multiple_entries() {
        let txs = parse_camt052(test_document()).unwrap();
        assert_eq!(txs.len(), 5);
    }

    #[test]
    fn raw_field_contains_metadata() {
        let txs = parse_camt052(test_document()).unwrap();
        let raw = &txs[0].raw;
        assert_eq!(raw["booking_date"], "2026-08-20");
        assert_eq!(raw["cdt_dbt_ind"], "DBIT");
        assert_eq!(raw["bk_tx_cd"], "NDDT+105+9208+888");
        assert_eq!(raw["end_to_end_id"], "E2E-REF-001");
        assert_eq!(raw["mandate_id"], "MNDT-001");
    }

    #[test]
    fn fallback_to_report_currency() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns="urn:iso:std:iso:20022:tech:xsd:camt.052.001.02">
  <BkToCstmrAcctRpt>
    <GrpHdr><MsgId>MSG002</MsgId></GrpHdr>
    <Rpt>
      <Acct><Id><IBAN>DE89370400440532013000</IBAN></Id><Ccy>USD</Ccy></Acct>
      <Ntry>
        <Amt>50.00</Amt>
        <CdtDbtInd>CRDT</CdtDbtInd>
        <Sts>BOOK</Sts>
        <BookgDt><Dt>2026-08-15</Dt></BookgDt>
        <ValDt><Dt>2026-08-15</Dt></ValDt>
        <AddtlNtryInf>TRANSFER</AddtlNtryInf>
      </Ntry>
    </Rpt>
  </BkToCstmrAcctRpt>
</Document>"#;
        let txs = parse_camt052(xml).unwrap();
        assert_eq!(txs[0].currency.as_str(), "USD");
    }

    #[test]
    fn reference_falls_back_to_end_to_end_id() {
        let txs = parse_camt052(test_document()).unwrap();
        assert_eq!(txs[1].reference.as_deref(), Some("SALARY-AUG-2026"));
    }

    #[test]
    fn invalid_xml_returns_error() {
        let result = parse_camt052(b"<not valid xml");
        assert!(result.is_err());
    }

    #[test]
    fn empty_document_returns_empty_vec() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns="urn:iso:std:iso:20022:tech:xsd:camt.052.001.02">
  <BkToCstmrAcctRpt>
    <GrpHdr><MsgId>MSG003</MsgId></GrpHdr>
    <Rpt>
      <Acct><Id><IBAN>DE89370400440532013000</IBAN></Id><Ccy>EUR</Ccy></Acct>
    </Rpt>
  </BkToCstmrAcctRpt>
</Document>"#;
        let txs = parse_camt052(xml).unwrap();
        assert!(txs.is_empty());
    }
}
