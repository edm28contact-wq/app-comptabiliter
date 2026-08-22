use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
pub struct CharlemagneLine {
    pub account: String,
    pub debit: String,
    pub credit: String,
    pub analytic_code: Option<String>,
    pub label: String,
}

#[derive(Serialize, Deserialize, Clone)]
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

#[derive(Clone)]
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
    match value.map(|value| value.trim().to_string()).filter(|value| !value.is_empty()) {
        Some(value) => value,
        None => {
            errors.push(format!("Champ requis manquant : {label}"));
            String::new()
        }
    }
}

fn amount_is_zero(value: &str) -> bool {
    value.replace(',', ".").parse::<f64>().map(|amount| amount.abs() < 0.005).unwrap_or(false)
}

pub fn prepare(input: PreparationInput) -> Result<PreparedCharlemagneEntry, Vec<String>> {
    let mut errors = Vec::new();
    let supplier = required(input.supplier, "fournisseur", &mut errors);
    let invoice_number = required(input.invoice_number, "numéro de facture", &mut errors);
    let date = required(input.invoice_date, "date de facture", &mut errors);
    let amount_ht = required(input.amount_ht, "montant HT", &mut errors);
    let amount_ttc = required(input.amount_ttc, "montant TTC", &mut errors);
    let supplier_account = required(input.supplier_account, "compte fournisseur", &mut errors);
    let expense_account = required(input.expense_account, "compte de charge", &mut errors);

    let amount_vat = input.amount_vat.unwrap_or_else(|| "0.00".to_string());
    let vat_account = if amount_is_zero(&amount_vat) {
        input.vat_account.unwrap_or_default()
    } else {
        required(input.vat_account, "compte TVA", &mut errors)
    };

    if !errors.is_empty() {
        return Err(errors);
    }

    let label = format!("{} - facture {}", supplier, invoice_number);
    let mut lines = vec![CharlemagneLine {
        account: expense_account,
        debit: amount_ht.clone(),
        credit: "0.00".to_string(),
        analytic_code: input.analytic_code.clone(),
        label: label.clone(),
    }];

    if !amount_is_zero(&amount_vat) {
        lines.push(CharlemagneLine {
            account: vat_account,
            debit: amount_vat.clone(),
            credit: "0.00".to_string(),
            analytic_code: None,
            label: label.clone(),
        });
    }

    lines.push(CharlemagneLine {
        account: supplier_account,
        debit: "0.00".to_string(),
        credit: amount_ttc.clone(),
        analytic_code: None,
        label: label.clone(),
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
        total: amount_ttc,
        document_path: input.document_path,
        lines,
        warnings,
        adapter_status: "adaptateur_non_configure".to_string(),
    })
}
