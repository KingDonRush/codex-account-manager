use crate::domain::MailProvider;

pub fn built_in_providers() -> Vec<MailProvider> {
    vec![
        provider(
            "gmail",
            "Gmail / Google Workspace",
            &["gmail.com", "googlemail.com"],
            "https://mail.google.com/mail/u/0/#inbox",
            &["google"],
        ),
        provider(
            "yahoo",
            "Yahoo",
            &["yahoo.com", "yahoo.com.br", "ymail.com"],
            "https://mail.yahoo.com/d/folders/1",
            &[],
        ),
        provider(
            "outlook",
            "Outlook.com",
            &["outlook.com"],
            "https://outlook.live.com/mail/0/inbox",
            &["microsoft"],
        ),
        provider(
            "hotmail",
            "Hotmail / Live",
            &["hotmail.com", "live.com", "msn.com"],
            "https://outlook.live.com/mail/0/inbox",
            &["outlook"],
        ),
        provider(
            "proton",
            "Proton Mail",
            &["proton.me", "protonmail.com"],
            "https://mail.proton.me/u/0/inbox",
            &[],
        ),
        provider(
            "tuta",
            "Tuta",
            &["tuta.com", "tutanota.com"],
            "https://mail.tuta.com",
            &["tutanota"],
        ),
        provider(
            "icloud",
            "iCloud Mail",
            &["icloud.com", "me.com", "mac.com"],
            "https://www.icloud.com/mail",
            &["apple"],
        ),
        provider("aol", "AOL", &["aol.com"], "https://mail.aol.com", &[]),
        provider(
            "zoho",
            "Zoho Mail",
            &["zoho.com", "zohomail.com"],
            "https://mail.zoho.com",
            &[],
        ),
        provider(
            "gmx",
            "GMX",
            &["gmx.com", "gmx.net", "gmx.de"],
            "https://www.gmx.com",
            &[],
        ),
        provider(
            "mail.com",
            "Mail.com",
            &["mail.com"],
            "https://www.mail.com",
            &[],
        ),
        provider(
            "fastmail",
            "Fastmail",
            &["fastmail.com"],
            "https://app.fastmail.com",
            &[],
        ),
        provider(
            "mailbox.org",
            "Mailbox.org",
            &["mailbox.org"],
            "https://login.mailbox.org/",
            &[],
        ),
        provider(
            "posteo",
            "Posteo",
            &["posteo.de", "posteo.net"],
            "https://posteo.de",
            &[],
        ),
        provider(
            "runbox",
            "Runbox",
            &["runbox.com"],
            "https://app.runbox.com",
            &[],
        ),
        provider(
            "migadu",
            "Migadu",
            &["migadu.com", "migadu.org"],
            "https://web.migadu.com",
            &[],
        ),
        provider(
            "yandex",
            "Yandex Mail",
            &["yandex.com", "yandex.ru"],
            "https://mail.yandex.com",
            &[],
        ),
        provider(
            "naver",
            "Naver",
            &["naver.com"],
            "https://mail.naver.com",
            &[],
        ),
        provider(
            "daum",
            "Daum / Kakao",
            &["daum.net", "hanmail.net"],
            "https://mail.daum.net",
            &["kakao"],
        ),
        provider("qq", "QQ Mail", &["qq.com"], "https://mail.qq.com", &[]),
        provider("163", "163 Mail", &["163.com"], "https://mail.163.com", &[]),
        provider("126", "126 Mail", &["126.com"], "https://mail.126.com", &[]),
        provider(
            "sina",
            "Sina Mail",
            &["sina.com", "sina.cn"],
            "https://mail.sina.com.cn",
            &[],
        ),
        provider("webde", "WEB.DE", &["web.de"], "https://web.de", &[]),
        provider(
            "orange",
            "Orange Mail",
            &["orange.fr"],
            "https://mail.orange.fr",
            &[],
        ),
    ]
}

fn provider(
    id: &str,
    display_name: &str,
    domains: &[&str],
    url: &str,
    aliases: &[&str],
) -> MailProvider {
    MailProvider {
        id: id.to_owned(),
        display_name: display_name.to_owned(),
        domains: domains.iter().map(|item| (*item).to_owned()).collect(),
        default_webmail_url: url.to_owned(),
        aliases: aliases.iter().map(|item| (*item).to_owned()).collect(),
        is_custom: false,
    }
}

pub fn resolve_provider<'a>(
    email: &str,
    providers: &'a [MailProvider],
) -> Option<&'a MailProvider> {
    let domain = email.rsplit_once('@')?.1.to_ascii_lowercase();
    providers
        .iter()
        .find(|provider| provider.domains.iter().any(|item| item == &domain))
}

#[cfg(test)]
mod tests {
    use super::{built_in_providers, resolve_provider};

    #[test]
    fn catalog_contains_the_twenty_five_expected_providers() {
        let providers = built_in_providers();
        assert_eq!(providers.len(), 25);
        assert_eq!(
            resolve_provider("person@yahoo.com.br", &providers).map(|item| item.id.as_str()),
            Some("yahoo")
        );
        assert_eq!(
            resolve_provider("person@proton.me", &providers).map(|item| item.id.as_str()),
            Some("proton")
        );
        assert!(resolve_provider("person@unknown.invalid", &providers).is_none());
    }
}
