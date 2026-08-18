//! Channel policy HTTP types and helpers (freeq `/api/v1/policy/…`).

use serde::{Deserialize, Serialize};

/// Result of `POST /api/v1/policy/{channel}/check`.
#[derive(Debug, Clone, Deserialize)]
pub struct PolicyCheck {
    pub channel: String,
    pub can_join: bool,
    pub status: String,
    pub requirements: Vec<RequirementStatus>,
    #[serde(default)]
    pub role: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RequirementStatus {
    pub requirement_type: String,
    pub description: String,
    pub satisfied: bool,
    #[serde(default)]
    pub action: Option<RequirementAction>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RequirementAction {
    pub action_type: String,
    #[serde(default)]
    pub url: Option<String>,
    pub label: String,
    #[serde(default)]
    pub accept_hash: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RulesResponse {
    #[allow(dead_code)]
    channel: String,
    #[allow(dead_code)]
    hash: String,
    text: String,
}

#[derive(Debug, Serialize)]
struct CheckRequest<'a> {
    did: &'a str,
}

/// True when a join denial is the authenticated-user policy gate (not guest auth).
pub fn is_policy_acceptance_denial(reason: &str) -> bool {
    let lower = reason.to_ascii_lowercase();
    lower.contains("policy acceptance") || lower.contains("policy accept")
}

/// Parse `Policy accepted for #channel — role: …` server notices.
pub fn parse_policy_accepted(text: &str) -> Option<String> {
    let text = text.trim();
    let lower = text.to_ascii_lowercase();
    if !lower.contains("policy accepted") {
        return None;
    }
    // `Policy accepted for #policytest — role: member. You may now JOIN.`
    let rest = text
        .split_once(" for ")
        .map(|(_, r)| r)
        .unwrap_or(text);
    let channel = rest
        .split_whitespace()
        .next()?
        .trim_matches(|c: char| c == '.' || c == ',');
    if channel.starts_with('#') || channel.starts_with('&') {
        Some(channel.to_string())
    } else {
        None
    }
}

/// Human-friendly title/subtitle for a requirement row.
pub fn friendly_requirement(req: &RequirementStatus) -> (String, String) {
    match req.requirement_type.as_str() {
        "accept" => (
            "Accept channel rules".into(),
            "Read and agree to the community guidelines".into(),
        ),
        "present" => {
            let cred_type = req
                .description
                .split("Credential:")
                .nth(1)
                .map(|s| s.trim().split_whitespace().next().unwrap_or(""))
                .unwrap_or("");
            let url = req.action.as_ref().and_then(|a| a.url.as_deref());
            match cred_type {
                "github_repo" => {
                    let repo = url_param(url, "repo");
                    (
                        "Verify GitHub access".into(),
                        repo.map(|r| format!("Collaborator on {r}"))
                            .unwrap_or_else(|| {
                                "Prove you're a collaborator on the linked repository".into()
                            }),
                    )
                }
                "github_membership" => {
                    let org = url_param(url, "org");
                    (
                        "Verify GitHub org membership".into(),
                        org.map(|o| format!("Member of {o}"))
                            .unwrap_or_else(|| {
                                "Prove you're a member of the linked organization".into()
                            }),
                    )
                }
                "bluesky_follower" => {
                    let target = url_param(url, "target");
                    (
                        "Verify Bluesky follow".into(),
                        target
                            .map(|t| format!("Follow @{t}"))
                            .unwrap_or_else(|| "Follow the required account on Bluesky".into()),
                    )
                }
                "channel_moderator" => (
                    "Moderator credential required".into(),
                    "A channel operator must appoint you as a moderator".into(),
                ),
                other if !other.is_empty() => (
                    format!("Present {} credential", other.replace('_', " ")),
                    "External verification required".into(),
                ),
                _ => (
                    req.description.clone(),
                    "External verification required".into(),
                ),
            }
        }
        "prove" => ("Provide proof".into(), req.description.clone()),
        _ => (req.description.clone(), String::new()),
    }
}

fn url_param(url: Option<&str>, param: &str) -> Option<String> {
    let url = url?;
    let full = if url.starts_with('/') {
        format!("https://x{url}")
    } else {
        url.to_string()
    };
    url::Url::parse(&full)
        .ok()
        .and_then(|u| u.query_pairs().find(|(k, _)| k == param).map(|(_, v)| v.into_owned()))
}

/// Build an absolute verification URL with callback + subject DID.
pub fn verification_url(raw: &str, api_base: &str, subject_did: &str) -> String {
    let full = if raw.starts_with("http://") || raw.starts_with("https://") {
        raw.to_string()
    } else {
        format!(
            "{}{}",
            api_base.trim_end_matches('/'),
            if raw.starts_with('/') { raw } else { "/" }
        )
    };
    let callback = format!("{}/api/v1/credentials/present", api_base.trim_end_matches('/'));
    if let Ok(mut u) = url::Url::parse(&full) {
        {
            let mut pairs: Vec<(String, String)> = u
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
            pairs.retain(|(k, _)| k != "callback" && k != "subject_did");
            pairs.push(("callback".into(), callback));
            pairs.push(("subject_did".into(), subject_did.to_string()));
            u.query_pairs_mut().clear();
            for (k, v) in pairs {
                u.query_pairs_mut().append_pair(&k, &v);
            }
        }
        return u.to_string();
    }
    // Fallback: append query params manually.
    let sep = if full.contains('?') { '&' } else { '?' };
    format!(
        "{full}{sep}callback={}&subject_did={}",
        urlencoding::encode(&callback),
        urlencoding::encode(subject_did)
    )
}

pub async fn fetch_check(api_base: &str, channel: &str, did: &str) -> Result<PolicyCheck, String> {
    let encoded = urlencoding::encode(channel);
    let url = format!("{}/api/v1/policy/{encoded}/check", api_base.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let res = client
        .post(&url)
        .json(&CheckRequest { did })
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(if body.is_empty() {
            format!("HTTP {status}")
        } else {
            body
        });
    }
    res.json::<PolicyCheck>()
        .await
        .map_err(|e| e.to_string())
}

pub async fn fetch_rules(api_base: &str, channel: &str) -> Result<String, String> {
    let encoded = urlencoding::encode(channel);
    let url = format!("{}/api/v1/policy/{encoded}/rules", api_base.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let res = client.get(&url).send().await.map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(if body.is_empty() {
            format!("HTTP {status}")
        } else {
            body
        });
    }
    let rules = res
        .json::<RulesResponse>()
        .await
        .map_err(|e| e.to_string())?;
    Ok(rules.text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_acceptance_denial_detection() {
        assert!(is_policy_acceptance_denial(
            "This channel requires policy acceptance — use POLICY <channel> ACCEPT"
        ));
        assert!(!is_policy_acceptance_denial(
            "This channel requires authentication — sign in to join"
        ));
    }

    #[test]
    fn parse_policy_accepted_notice() {
        assert_eq!(
            parse_policy_accepted(
                "Policy accepted for #policytest — role: member. You may now JOIN."
            ),
            Some("#policytest".into())
        );
    }
}
