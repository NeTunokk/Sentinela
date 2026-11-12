use sentinela_core::model::Report;
use sentinela_core::Result;

/// Serialização determinística (pretty) do relatório canônico.
pub fn serialize(report: &Report) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec_pretty(report)?)
}
