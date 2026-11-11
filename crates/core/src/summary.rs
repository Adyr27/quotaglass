//! One ring per service on the pill: the best available account's tightest limit.

use crate::model::Service;
use crate::view::{AccountView, CardState, Freshness, ServiceView};

/// One `ServiceView` per service with at least one visible account, in `Service` order.
pub fn build(accounts: &[AccountView]) -> Vec<ServiceView> {
    [Service::Claude, Service::Codex, Service::Antigravity]
        .into_iter()
        .filter_map(|service| {
            let visible: Vec<&AccountView> = accounts
                .iter()
                .filter(|a| a.service == service && !a.hidden)
                .collect();
            if visible.is_empty() {
                return None;
            }
            // Counted: a real, current number. A headline whose window already reset is not one.
            let counted: Vec<f64> = visible
                .iter()
                .filter_map(|a| a.headline.as_ref())
                .filter(|h| h.freshness != Freshness::Stale)
                .map(|h| h.used_percent)
                .collect();
            Some(ServiceView {
                service,
                used_percent: counted.iter().copied().reduce(f64::min),
                all_exhausted: !counted.is_empty() && counted.iter().all(|&p| p >= 100.0),
                accounts: visible.len() as u32,
                rate_limited: visible.iter().any(|a| a.state == CardState::RateLimited),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::BucketView;

    fn bucket(used: f64, freshness: Freshness) -> BucketView {
        BucketView {
            name: "5-hour".into(),
            used_percent: used,
            window_secs: Some(18_000),
            resets_at_ms: None,
            limit_reached: used >= 100.0,
            freshness,
            passed_percent: None,
            fast_burn: false,
        }
    }

    fn acct(id: &str, service: Service, used: Option<f64>) -> AccountView {
        let headline = used.map(|u| bucket(u, Freshness::Live));
        AccountView {
            id: id.into(),
            service,
            label: id.into(),
            plan: None,
            source: None,
            freshness: Freshness::Live,
            age_secs: None,
            buckets: headline.iter().cloned().collect(),
            headline,
            state: if used.is_some() {
                CardState::Ok
            } else {
                CardState::NoReadingYet
            },
            reason: None,
            room_left: None,
            best_now: false,
            banked_resets: None,
            profile_id: None,
            current_login: false,
            hidden: false,
        }
    }

    #[test]
    fn best_available_is_the_lowest() {
        let s = build(&[
            acct("a", Service::Claude, Some(70.0)),
            acct("b", Service::Claude, Some(20.0)),
        ]);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].used_percent, Some(20.0));
        assert_eq!(s[0].accounts, 2);
        assert!(!s[0].all_exhausted);
    }

    #[test]
    fn hidden_accounts_do_not_count() {
        let mut hidden = acct("b", Service::Claude, Some(10.0));
        hidden.hidden = true;
        let s = build(&[acct("a", Service::Claude, Some(60.0)), hidden]);
        assert_eq!(s[0].used_percent, Some(60.0));
        assert_eq!(s[0].accounts, 1);

        let mut only = acct("c", Service::Codex, Some(10.0));
        only.hidden = true;
        assert!(
            build(&[only]).is_empty(),
            "a service with only hidden accounts has no slot"
        );
    }

    #[test]
    fn no_reading_gives_none() {
        let s = build(&[acct("a", Service::Codex, None)]);
        assert_eq!(s[0].used_percent, None);
        assert_eq!(s[0].accounts, 1);
        assert!(!s[0].all_exhausted);
    }

    #[test]
    fn all_exhausted_needs_every_account() {
        let s = build(&[
            acct("a", Service::Claude, Some(100.0)),
            acct("b", Service::Claude, Some(100.0)),
        ]);
        assert!(s[0].all_exhausted);
        assert_eq!(s[0].used_percent, Some(100.0));
    }

    #[test]
    fn one_exhausted_one_fine_is_not_exhausted() {
        let s = build(&[
            acct("a", Service::Claude, Some(100.0)),
            acct("b", Service::Claude, Some(40.0)),
        ]);
        assert!(!s[0].all_exhausted);
        assert_eq!(s[0].used_percent, Some(40.0));
    }

    #[test]
    fn rate_limited_flag() {
        let mut limited = acct("a", Service::Claude, Some(30.0));
        limited.state = CardState::RateLimited;
        let s = build(&[limited, acct("b", Service::Claude, Some(50.0))]);
        assert!(s[0].rate_limited);
        assert_eq!(
            s[0].used_percent,
            Some(30.0),
            "a rate-limited card keeps its reading"
        );

        let s = build(&[acct("c", Service::Claude, Some(50.0))]);
        assert!(!s[0].rate_limited);
    }

    #[test]
    fn services_in_order_and_only_with_accounts() {
        let s = build(&[
            acct("g", Service::Antigravity, Some(5.0)),
            acct("c", Service::Claude, Some(5.0)),
        ]);
        let order: Vec<Service> = s.iter().map(|v| v.service).collect();
        assert_eq!(order, vec![Service::Claude, Service::Antigravity]);
        assert!(build(&[]).is_empty());
    }

    #[test]
    fn stale_headline_does_not_count() {
        let mut stale = acct("a", Service::Claude, Some(10.0));
        stale.headline = Some(bucket(10.0, Freshness::Stale));
        let s = build(&[stale, acct("b", Service::Claude, Some(55.0))]);
        assert_eq!(s[0].used_percent, Some(55.0));
        assert_eq!(s[0].accounts, 2);
    }
}
