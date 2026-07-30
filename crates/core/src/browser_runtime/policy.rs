#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserActionRisk {
    Low,
    Consequential,
    SensitiveInput,
}

pub fn classify_action_risk(
    action: &str,
    role: Option<&str>,
    name: Option<&str>,
    href: Option<&str>,
    input_type: Option<&str>,
) -> BrowserActionRisk {
    let action = action.trim().to_ascii_lowercase();
    let role = role.unwrap_or_default().to_ascii_lowercase();
    let name = name.unwrap_or_default().to_ascii_lowercase();
    let input_type = input_type.unwrap_or_default().to_ascii_lowercase();
    let sensitive_words = [
        "password",
        "passcode",
        "verification code",
        "otp",
        "credit card",
        "card number",
        "cvv",
        "payment",
        "authorize",
        "passkey",
        "密码",
        "验证码",
        "支付",
    ];
    if action == "type"
        && (matches!(input_type.as_str(), "password" | "file")
            || sensitive_words.iter().any(|word| name.contains(word)))
    {
        return BrowserActionRisk::SensitiveInput;
    }
    if matches!(action.as_str(), "close_tab" | "close_session") {
        return BrowserActionRisk::Consequential;
    }
    if matches!(
        action.as_str(),
        "create_session"
            | "list_sessions"
            | "list_tabs"
            | "open_tab"
            | "activate_tab"
            | "navigate"
            | "observe"
            | "scroll"
            | "wait_for"
            | "go_back"
            | "go_forward"
            | "reload"
    ) {
        return BrowserActionRisk::Low;
    }
    let consequential_words = [
        "submit",
        "send",
        "publish",
        "post",
        "delete",
        "remove",
        "merge",
        "purchase",
        "buy",
        "pay",
        "confirm",
        "authorize",
        "allow",
        "upload",
        "download",
        "提交",
        "发送",
        "发布",
        "删除",
        "合并",
        "购买",
        "付款",
        "授权",
    ];
    if consequential_words.iter().any(|word| name.contains(word)) || action == "press" {
        return BrowserActionRisk::Consequential;
    }
    if action == "click" && role == "link" && href.is_some_and(|value| value.starts_with("http")) {
        return BrowserActionRisk::Low;
    }
    if action == "type"
        && matches!(
            input_type.as_str(),
            "search" | "text" | "email" | "url" | ""
        )
    {
        return BrowserActionRisk::Low;
    }
    BrowserActionRisk::Consequential
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_separates_normal_navigation_from_external_effects() {
        assert_eq!(
            classify_action_risk("navigate", None, None, None, None),
            BrowserActionRisk::Low
        );
        assert_eq!(
            classify_action_risk("click", Some("button"), Some("Pay now"), None, None),
            BrowserActionRisk::Consequential
        );
        assert_eq!(
            classify_action_risk(
                "click",
                Some("link"),
                Some("Delete account"),
                Some("https://example.com/account/delete"),
                None
            ),
            BrowserActionRisk::Consequential
        );
        assert_eq!(
            classify_action_risk(
                "type",
                Some("textbox"),
                Some("Password"),
                None,
                Some("password")
            ),
            BrowserActionRisk::SensitiveInput
        );
        assert_eq!(
            classify_action_risk("close_tab", None, None, None, None),
            BrowserActionRisk::Consequential
        );
        assert_eq!(
            classify_action_risk("close_session", None, None, None, None),
            BrowserActionRisk::Consequential
        );
    }
}
