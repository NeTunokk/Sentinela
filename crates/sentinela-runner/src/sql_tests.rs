use std::time::{Duration, Instant};

use sentinela_core::config::TestDefinition;
use sentinela_core::model::{ObservedValue, TestOutcome, TestStatus};
use sentinela_core::{Result, SentinelaError};
use tokio_postgres::error::SqlState;
use tokio_postgres::{Client, NoTls};
use tracing::debug;

use crate::docker::SandboxSpec;

/// Executa todos os testes em sequência. Cada teste roda em uma transação
/// própria que é sempre revertida, mesmo quando ele escreve.
pub async fn run_sql_tests(
    spec: &SandboxSpec,
    host: &str,
    tests: &[TestDefinition],
    default_timeout_ms: u64,
) -> Result<Vec<TestOutcome>> {
    let mut client = connect(spec, host).await?;
    let mut outcomes = Vec::with_capacity(tests.len());
    for test in tests {
        outcomes.push(run_one(&mut client, test, default_timeout_ms).await);
    }
    Ok(outcomes)
}

async fn connect(spec: &SandboxSpec, host: &str) -> Result<Client> {
    let mut config = tokio_postgres::Config::new();
    config
        .host(host)
        .port(5432)
        .user(&spec.postgres_user)
        .password(spec.password())
        .dbname(&spec.database)
        .connect_timeout(Duration::from_secs(10))
        .ssl_mode(tokio_postgres::config::SslMode::Disable);

    let (client, connection) = config.connect(NoTls).await.map_err(|error| {
        SentinelaError::Docker(format!(
            "não foi possível conectar ao PostgreSQL do sandbox{}",
            sqlstate_suffix(error.code())
        ))
    })?;

    tokio::spawn(async move {
        if let Err(error) = connection.await {
            debug!(
                sqlstate = error.code().map(SqlState::code).unwrap_or("desconhecido"),
                "conexão de teste PostgreSQL encerrada"
            );
        }
    });

    Ok(client)
}

async fn run_one(
    client: &mut Client,
    test: &TestDefinition,
    default_timeout_ms: u64,
) -> TestOutcome {
    let started = Instant::now();
    let timeout_ms = test.timeout_ms.unwrap_or(default_timeout_ms);

    let transaction = match client.transaction().await {
        Ok(transaction) => transaction,
        Err(error) => {
            return error_outcome(
                test,
                started,
                format!(
                    "não foi possível iniciar transação{}",
                    sqlstate_suffix(error.code())
                ),
            );
        }
    };

    let timeout_statement = format!("SET LOCAL statement_timeout = {timeout_ms}");
    if let Err(error) = transaction.batch_execute(&timeout_statement).await {
        let summary = sqlstate_suffix(error.code());
        if let Err(rollback_error) = transaction.rollback().await {
            debug!(
                sqlstate = rollback_error
                    .code()
                    .map(SqlState::code)
                    .unwrap_or("desconhecido"),
                "falha ao reverter transação de teste"
            );
        }
        return error_outcome(
            test,
            started,
            format!("não foi possível aplicar statement_timeout{summary}"),
        );
    }

    let wrapped = wrap_scalar_query(&test.sql);
    let query_result = transaction.query(&wrapped, &[]).await;

    let evaluation = match query_result {
        Ok(rows) => match rows.as_slice() {
            [] => Err(("nenhuma linha retornada".to_string(), TestStatus::Failed)),
            [_, _, ..] => Err((
                "consulta retornou mais de uma linha".to_string(),
                TestStatus::Error,
            )),
            [row] => {
                let raw = row.try_get::<_, Option<String>>(0);
                match raw {
                    Ok(raw) => match parse_observed(raw) {
                        Ok(observed) => match test.expect.evaluate(observed.as_ref()) {
                            Ok(()) => Ok(observed.map(|value| (value, TestStatus::Passed))),
                            Err(message) => Err((message, TestStatus::Failed)),
                        },
                        Err(message) => Err((message, TestStatus::Error)),
                    },
                    Err(error) => Err((
                        format!(
                            "não foi possível ler o resultado{}",
                            sqlstate_suffix(error.code())
                        ),
                        TestStatus::Error,
                    )),
                }
            }
        },
        Err(error) => Err((
            format!("erro ao executar SQL{}", sqlstate_suffix(error.code())),
            TestStatus::Error,
        )),
    };

    if let Err(error) = transaction.rollback().await {
        debug!(
            sqlstate = error.code().map(SqlState::code).unwrap_or("desconhecido"),
            "falha ao reverter transação de teste"
        );
        return error_outcome(
            test,
            started,
            format!(
                "falha ao reverter transação{}",
                sqlstate_suffix(error.code())
            ),
        );
    }

    let duration_ms = millis(started.elapsed());
    match evaluation {
        Ok(Some((value, status))) => TestOutcome {
            name: test.name.clone(),
            status,
            value: Some(value),
            duration_ms,
            message: None,
        },
        Ok(None) => TestOutcome {
            name: test.name.clone(),
            status: TestStatus::Passed,
            value: None,
            duration_ms,
            message: None,
        },
        Err((message, status)) => TestOutcome {
            name: test.name.clone(),
            status,
            value: None,
            duration_ms,
            message: Some(message),
        },
    }
}

fn error_outcome(test: &TestDefinition, started: Instant, message: String) -> TestOutcome {
    TestOutcome {
        name: test.name.clone(),
        status: TestStatus::Error,
        value: None,
        duration_ms: millis(started.elapsed()),
        message: Some(message),
    }
}

/// Envolve o SQL em uma subconsulta escalar e converte o primeiro valor para
/// texto, permitindo suportar int, bigint, numeric, float e boolean sem
/// depender de tipos opcionais do tokio-postgres.
fn wrap_scalar_query(sql: &str) -> String {
    let trimmed = sql.trim().trim_end_matches(';').trim_end();
    format!("SELECT v::text FROM ({trimmed}) AS sentinela_value(v)")
}

fn parse_observed(raw: Option<String>) -> std::result::Result<Option<ObservedValue>, String> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let value = raw.trim();
    if value.eq_ignore_ascii_case("t") || value.eq_ignore_ascii_case("true") {
        return Ok(Some(ObservedValue::Bool(true)));
    }
    if value.eq_ignore_ascii_case("f") || value.eq_ignore_ascii_case("false") {
        return Ok(Some(ObservedValue::Bool(false)));
    }
    if let Ok(integer) = value.parse::<i64>() {
        return Ok(Some(ObservedValue::Int(integer)));
    }
    if let Ok(float) = value.parse::<f64>() {
        if float.is_finite() {
            return Ok(Some(ObservedValue::Float(float)));
        }
    }
    Err("resultado não é booleano nem numérico".to_string())
}

fn sqlstate_suffix(code: Option<&SqlState>) -> String {
    code.map_or_else(String::new, |code| format!(" (SQLSTATE {})", code.code()))
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_and_strips_semicolon() {
        assert_eq!(
            wrap_scalar_query("SELECT count(*) FROM users;"),
            "SELECT v::text FROM (SELECT count(*) FROM users) AS sentinela_value(v)"
        );
    }

    #[test]
    fn parses_scalar_values() {
        assert_eq!(
            parse_observed(Some("42".into())).unwrap(),
            Some(ObservedValue::Int(42))
        );
        assert_eq!(
            parse_observed(Some("1.5".into())).unwrap(),
            Some(ObservedValue::Float(1.5))
        );
        assert_eq!(
            parse_observed(Some("true".into())).unwrap(),
            Some(ObservedValue::Bool(true))
        );
        assert_eq!(parse_observed(None).unwrap(), None);
        assert!(parse_observed(Some("texto".into())).is_err());
        assert!(parse_observed(Some("NaN".into())).is_err());
    }
}
