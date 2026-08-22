use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CharlemagneLine {
    pub account: String,
    pub debit: String,
    pub credit: String,
    pub analytic_code: Option<String>,
    pub label: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PreparedCharlemagneEntry {
    pub date: String,
    pub reference: String,
    pub supplier: String,
    pub invoice_number: String,
    pub currency: String,
    pub total: String,
    pub document_path: Option<String>,
    pub lines: Vec<CharlemagneLine>,
    pub warnings: Vec<String>,
    pub adapter_status: String,
}

#[derive(Clone, Default)]
pub struct PreparationInput {
    pub supplier: Option<String>,
    pub invoice_number: Option<String>,
    pub invoice_date: Option<String>,
    pub amount_ht: Option<String>,
    pub amount_vat: Option<String>,
    pub amount_ttc: Option<String>,
    pub supplier_account: Option<String>,
    pub expense_account: Option<String>,
    pub vat_account: Option<String>,
    pub analytic_code: Option<String>,
    pub document_path: Option<String>,
}

fn required(value: Option<String>, label: &str, errors: &mut Vec<String>) -> String {
    match value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        Some(value) => value,
        None => {
            errors.push(format!("Champ requis manquant : {label}"));
            String::new()
        }
    }
}

fn parse_amount(value: &str, label: &str, errors: &mut Vec<String>) -> Option<f64> {
    let cleaned = value
        .replace('€', "")
        .replace("EUR", "")
        .replace('\u{00a0}', "")
        .replace('\u{202f}', "")
        .replace(' ', "");

    let normalized = if cleaned.contains(',') {
        cleaned.replace('.', "").replace(',', ".")
    } else {
        cleaned
    };

    match normalized.parse::<f64>() {
        Ok(amount) if amount.is_finite() && amount >= 0.0 => Some(amount),
        Ok(_) => {
            errors.push(format!("Montant invalide ou négatif : {label}"));
            None
        }
        Err(_) => {
            errors.push(format!("Montant non numérique : {label}"));
            None
        }
    }
}

fn format_amount(amount: f64) -> String {
    format!("{amount:.2}")
}

fn amount_is_zero(amount: f64) -> bool {
    amount.abs() < 0.005
}

pub fn prepare(input: PreparationInput) -> Result<PreparedCharlemagneEntry, Vec<String>> {
    let mut errors = Vec::new();
    let supplier = required(input.supplier, "fournisseur", &mut errors);
    let invoice_number = required(input.invoice_number, "numéro de facture", &mut errors);
    let date = required(input.invoice_date, "date de facture", &mut errors);
    let amount_ht_raw = required(input.amount_ht, "montant HT", &mut errors);
    let amount_ttc_raw = required(input.amount_ttc, "montant TTC", &mut errors);
    let supplier_account = required(input.supplier_account, "compte fournisseur", &mut errors);
    let expense_account = required(input.expense_account, "compte de charge", &mut errors);
    let amount_vat_raw = input.amount_vat.unwrap_or_else(|| "0.00".to_string());

    let amount_ht = if amount_ht_raw.is_empty() { None } else { parse_amount(&amount_ht_raw, "HT", &mut errors) };
    let amount_vat = parse_amount(&amount_vat_raw, "TVA", &mut errors);
    let amount_ttc = if amount_ttc_raw.is_empty() { None } else { parse_amount(&amount_ttc_raw, "TTC", &mut errors) };

    let vat_account = match amount_vat {
        Some(vat) if amount_is_zero(vat) => input.vat_account.unwrap_or_default(),
        Some(_) => required(input.vat_account, "compte TVA", &mut errors),
        None => String::new(),
    };

    if let (Some(ht), Some(vat), Some(ttc)) = (amount_ht, amount_vat, amount_ttc) {
        if ((ht + vat) - ttc).abs() > 0.02 {
            errors.push(format!(
                "Écriture déséquilibrée : HT {} + TVA {} ≠ TTC {}",
                format_amount(ht),
                format_amount(vat),
                format_amount(ttc)
            ));
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    let amount_ht = amount_ht.expect("amount_ht checked above");
    let amount_vat = amount_vat.expect("amount_vat checked above");
    let amount_ttc = amount_ttc.expect("amount_ttc checked above");
    let amount_ht_text = format_amount(amount_ht);
    let amount_vat_text = format_amount(amount_vat);
    let amount_ttc_text = format_amount(amount_ttc);
    let label = format!("{} - facture {}", supplier, invoice_number);

    let mut lines = vec![CharlemagneLine {
        account: expense_account,
        debit: amount_ht_text,
        credit: "0.00".to_string(),
        analytic_code: input.analytic_code.clone(),
        label: label.clone(),
    }];

    if !amount_is_zero(amount_vat) {
        lines.push(CharlemagneLine {
            account: vat_account,
            debit: amount_vat_text,
            credit: "0.00".to_string(),
            analytic_code: None,
            label: label.clone(),
        });
    }

    lines.push(CharlemagneLine {
        account: supplier_account,
        debit: "0.00".to_string(),
        credit: amount_ttc_text.clone(),
        analytic_code: None,
        label,
    });

    let mut warnings = Vec::new();
    if input.analytic_code.is_none() {
        warnings.push("Aucun code analytique validé.".to_string());
    }
    warnings.push("Écriture intermédiaire : aucun format Charlemagne spécifique n'est encore généré.".to_string());

    Ok(PreparedCharlemagneEntry {
        date,
        reference: invoice_number.clone(),
        supplier,
        invoice_number,
        currency: "EUR".to_string(),
        total: amount_ttc_text,
        document_path: input.document_path,
        lines,
        warnings,
        adapter_status: "adaptateur_non_configure".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_input() -> PreparationInput {
        PreparationInput {
            supplier: Some("EDF".to_string()),
            invoice_number: Some("874521".to_string()),
            invoice_date: Some("18/08/2026".to_string()),
            amount_ht: Some("1000,00".to_string()),
            amount_vat: Some("200,00".to_string()),
            amount_ttc: Some("1200,00".to_string()),
            supplier_account: Some("401EDF".to_string()),
            expense_account: Some("606100".to_string()),
            vat_account: Some("445660".to_string()),
            analytic_code: Some("COLLEGE".to_string()),
            document_path: Some(r"C:\\Archives\\facture.pdf".to_string()),
        }
    }

    #[test]
    fn prepares_balanced_entry() {
        let entry = prepare(valid_input()).expect("balanced input should be accepted");
        assert_eq!(entry.total, "1200.00");
        assert_eq!(entry.lines.len(), 3);
        assert_eq!(entry.lines[0].debit, "1000.00");
        assert_eq!(entry.lines[1].debit, "200.00");
        assert_eq!(entry.lines[2].credit, "1200.00");
    }

    #[test]
    fn accepts_french_thousands_separator() {
        let mut input = valid_input();
        input.amount_ht = Some("1.000,00".to_string());
        input.amount_vat = Some("200,00".to_string());
        input.amount_ttc = Some("1.200,00".to_string());
        let entry = prepare(input).expect("French thousands separators should be accepted");
        assert_eq!(entry.total, "1200.00");
    }

    #[test]
    fn rejects_unbalanced_entry() {
        let mut input = valid_input();
        input.amount_ttc = Some("1250.00".to_string());
        let errors = prepare(input).expect_err("unbalanced input must be rejected");
        assert!(errors.iter().any(|error| error.contains("déséquilibrée")));
    }

    #[test]
    fn rejects_missing_supplier_account() {
        let mut input = valid_input();
        input.supplier_account = None;
        let errors = prepare(input).expect_err("missing supplier account must be rejected");
        assert!(errors.iter().any(|error| error.contains("compte fournisseur")));
    }

    #[test]
    fn supports_invoice_without_vat() {
        let mut input = valid_input();
        input.amount_vat = Some("0".to_string());
        input.amount_ht = Some("1200".to_string());
        input.vat_account = None;
        let entry = prepare(input).expect("zero VAT should not require a VAT account");
        assert_eq!(entry.lines.len(), 2);
    }

    #[test]
    fn rejects_non_numeric_amounts() {
        let mut input = valid_input();
        input.amount_ht = Some("mille".to_string());
        let errors = prepare(input).expect_err("non numeric amount must be rejected");
        assert!(errors.iter().any(|error| error.contains("non numérique")));
    }
}
