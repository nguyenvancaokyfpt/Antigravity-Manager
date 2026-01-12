//! Accounts Handler - Check Token API
//!
//! Provides endpoint for checking account status and refreshing quotas.

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use serde::Serialize;
use tracing::{error, info, warn};

use crate::models::{Account, QuotaData};
use crate::modules;
use crate::proxy::server::AppState;

/// Response structure for check-token API
#[derive(Debug, Serialize)]
pub struct CheckTokenResponse {
    /// Whether the operation succeeded
    pub success: bool,
    /// Total number of accounts
    pub total_accounts: usize,
    /// Number of accounts successfully refreshed
    pub refreshed: usize,
    /// Number of accounts that failed to refresh
    pub failed: usize,
    /// List of all accounts with their current quota info
    pub accounts: Vec<AccountInfo>,
    /// Summary statistics
    pub summary: AccountSummary,
}

/// Simplified account info for API response
#[derive(Debug, Serialize)]
pub struct AccountInfo {
    pub id: String,
    pub email: String,
    pub name: Option<String>,
    pub subscription_tier: Option<String>,
    pub disabled: bool,
    pub proxy_disabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota: Option<QuotaInfo>,
    /// Refresh status for this account
    pub refresh_status: RefreshStatus,
}

/// Quota info for API response
#[derive(Debug, Serialize)]
pub struct QuotaInfo {
    pub last_updated: i64,
    pub is_forbidden: bool,
    pub models: Vec<ModelQuotaInfo>,
}

/// Model quota info
#[derive(Debug, Serialize)]
pub struct ModelQuotaInfo {
    pub name: String,
    pub percentage: i32,
    pub reset_time: String,
}

/// Refresh status for individual account
#[derive(Debug, Serialize)]
pub struct RefreshStatus {
    pub refreshed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Summary statistics
#[derive(Debug, Serialize)]
pub struct AccountSummary {
    pub pro_count: usize,
    pub ultra_count: usize,
    pub free_count: usize,
    pub active_count: usize,
    pub disabled_count: usize,
}

/// Handle check-token API - returns account info and refreshes all quotas
pub async fn handle_check_token(State(_state): State<AppState>) -> Response {
    info!("📊 Check-token API called - refreshing all quotas...");

    // 1. Load all accounts from the main account module
    let accounts_result = modules::account::list_accounts();
    let mut accounts = match accounts_result {
        Ok(accs) => accs,
        Err(e) => {
            error!("Failed to load accounts: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "success": false,
                    "error": format!("Failed to load accounts: {}", e)
                })),
            )
                .into_response();
        }
    };

    let total_accounts = accounts.len();
    let mut refreshed = 0;
    let mut failed = 0;
    let mut account_infos: Vec<AccountInfo> = Vec::with_capacity(total_accounts);

    // 2. Refresh quota for each account
    for account in accounts.iter_mut() {
        let refresh_result = refresh_account_quota(account).await;

        let (refresh_status, quota_info) = match refresh_result {
            Ok(q) => {
                refreshed += 1;
                (
                    RefreshStatus {
                        refreshed: true,
                        error: None,
                    },
                    Some(convert_quota_info(&q)),
                )
            }
            Err(e) => {
                failed += 1;
                warn!("Failed to refresh quota for {}: {}", account.email, e);
                (
                    RefreshStatus {
                        refreshed: false,
                        error: Some(e),
                    },
                    account.quota.as_ref().map(|q| convert_quota_info(q)),
                )
            }
        };

        account_infos.push(AccountInfo {
            id: account.id.clone(),
            email: account.email.clone(),
            name: account.name.clone(),
            subscription_tier: account
                .quota
                .as_ref()
                .and_then(|q| q.subscription_tier.clone()),
            disabled: account.disabled,
            proxy_disabled: account.proxy_disabled,
            quota: quota_info,
            refresh_status,
        });
    }

    // 3. Calculate summary statistics
    let summary = calculate_summary(&account_infos);

    info!(
        "✅ Check-token completed: {}/{} accounts refreshed successfully",
        refreshed, total_accounts
    );

    // 4. Return response
    let response = CheckTokenResponse {
        success: true,
        total_accounts,
        refreshed,
        failed,
        accounts: account_infos,
        summary,
    };

    Json(response).into_response()
}

/// Refresh quota for a single account
async fn refresh_account_quota(account: &mut Account) -> Result<QuotaData, String> {
    // Skip disabled accounts
    if account.disabled {
        return Err("Account is disabled".to_string());
    }

    // Fetch quota using the quota module
    let result = modules::account::fetch_quota_with_retry(account);

    match result.await {
        Ok(quota) => {
            // Save the updated quota
            account.quota = Some(quota.clone());
            if let Err(e) = modules::account::save_account(account) {
                warn!("Failed to save account after quota refresh: {}", e);
            }
            Ok(quota)
        }
        Err(e) => Err(format!("{:?}", e)),
    }
}

/// Convert QuotaData to QuotaInfo for API response
fn convert_quota_info(quota: &QuotaData) -> QuotaInfo {
    QuotaInfo {
        last_updated: quota.last_updated,
        is_forbidden: quota.is_forbidden,
        models: quota
            .models
            .iter()
            .map(|m| ModelQuotaInfo {
                name: m.name.clone(),
                percentage: m.percentage,
                reset_time: m.reset_time.clone(),
            })
            .collect(),
    }
}

/// Calculate summary statistics from account infos
fn calculate_summary(accounts: &[AccountInfo]) -> AccountSummary {
    let mut pro_count = 0;
    let mut ultra_count = 0;
    let mut free_count = 0;
    let mut active_count = 0;
    let mut disabled_count = 0;

    for acc in accounts {
        if acc.disabled {
            disabled_count += 1;
        } else {
            active_count += 1;
        }

        if let Some(tier) = &acc.subscription_tier {
            let tier_lower = tier.to_lowercase();
            if tier_lower.contains("ultra") {
                ultra_count += 1;
            } else if tier_lower.contains("pro") {
                pro_count += 1;
            } else {
                free_count += 1;
            }
        } else {
            free_count += 1;
        }
    }

    AccountSummary {
        pro_count,
        ultra_count,
        free_count,
        active_count,
        disabled_count,
    }
}
