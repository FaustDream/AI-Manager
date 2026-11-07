//! WorkBuddy 国内版 / 国际版：读 `.info` 登录票据，并按「账号档案」换登录态。
//!
//! 模块结构：`crypto`（.info 字段解密）、`profile`（档案与切换）、
//! `provider`（trait 实现：扫描/签到/积分/旅行接口层）、`growth`（成长中心与
//! 旅行数据面）、`endpoints`（平台端点常量）。本文件只保留组装与测试。

mod crypto;
mod endpoints;
mod growth;
mod profile;
mod provider;

pub use crypto::open_field;
pub use growth::{RewardCredits, RewardPackage};
pub use profile::{switch_to, WorkBuddyProvider};

#[cfg(test)]
mod tests {
    use super::crypto::{wb_aad, wb_key_bytes, wb_key_id, WbCipher16};
    use super::endpoints::{TRAVEL_BASE, TRAVEL_CLAIM, TRAVEL_DEPART, USER_AGENT};
    use super::growth::{normalize_travel_record, normalize_travel_status, travel_location};
    use super::*;
    use crate::app::accounts::credential::Credential;
    use crate::app::activity::TravelState;
    use crate::app::fsx;
    use crate::app::net::MockHttp;
    use crate::app::providers::Provider;
    use aes_gcm::aead::consts::U16;
    use aes_gcm::aead::Aead;
    use aes_gcm::aead::KeyInit;
    use base64::Engine;
    use serde_json::{json, Value};
    use std::path::Path;

    fn provider(dir: &Path) -> WorkBuddyProvider {
        WorkBuddyProvider::at(
            false,
            dir.join("auth"),
            dir.join("client"),
            dir.join("profiles"),
        )
    }

    /// 用客户端的固定密钥造一个 `$wbEncrypted` 字段（等价于 v1 的 `seal_field`）
    fn seal_with(nonce: &[u8], value: &str) -> Value {
        let key_id = wb_key_id();
        let aad = wb_aad(b"WBEV1", 2, &key_id, 1);
        let sealed = match nonce.len() {
            12 => aes_gcm::Aes256Gcm::new_from_slice(&wb_key_bytes())
                .unwrap()
                .encrypt(
                    aes_gcm::Nonce::from_slice(nonce),
                    aes_gcm::aead::Payload {
                        msg: value.as_bytes(),
                        aad: &aad,
                    },
                )
                .unwrap(),
            16 => WbCipher16::new_from_slice(&wb_key_bytes())
                .unwrap()
                .encrypt(
                    aes_gcm::Nonce::<U16>::from_slice(nonce),
                    aes_gcm::aead::Payload {
                        msg: value.as_bytes(),
                        aad: &aad,
                    },
                )
                .unwrap(),
            _ => unreachable!(),
        };
        let (ciphertext, tag) = sealed.split_at(sealed.len() - 16);
        let envelope = json!({
            "suite": 1,
            "keyId": key_id,
            "nonce": base64::engine::general_purpose::STANDARD.encode(nonce),
            "authTag": base64::engine::general_purpose::STANDARD.encode(tag),
            "ciphertext": base64::engine::general_purpose::STANDARD.encode(ciphertext),
        });
        json!({
            "$wbEncrypted": 1,
            "envelope": base64::engine::general_purpose::STANDARD
                .encode(serde_json::to_vec(&envelope).unwrap()),
        })
    }

    /// 客户端实际写的票据用 12 字节 nonce
    fn seal(value: &str) -> Value {
        seal_with(&[5u8; 12], value)
    }

    fn ticket(uid: &str, token: &str, name: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "auth": {
                "accessToken": seal(token),
                "refreshToken": seal("rt"),
                "expiresAt": 1_700_000_000_000i64,
                "domain": "www.workbuddy.cn",
            },
            "account": { "uid": uid, "nickname": seal(name) },
        }))
        .unwrap()
    }

    #[test]
    fn encrypted_field_roundtrip() {
        let field = seal("17679130626");
        assert_eq!(open_field(&field).unwrap(), "17679130626");
    }

    #[test]
    fn both_nonce_lengths_are_supported() {
        // 客户端 12 字节；v1 的 seal_field 写 16 字节。两种都出现过
        assert_eq!(open_field(&seal_with(&[1u8; 12], "a")).unwrap(), "a");
        assert_eq!(open_field(&seal_with(&[1u8; 16], "b")).unwrap(), "b");
    }

    #[test]
    fn plain_field_passes_through() {
        assert_eq!(open_field(&json!("plain")).unwrap(), "plain");
        assert_eq!(open_field(&Value::Null).unwrap(), "");
    }

    #[test]
    fn headers_prefer_account_ua_and_fall_back_to_constant() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = provider(dir.path());

        let mut cred = Credential {
            token: "tk".into(),
            ..Default::default()
        };
        cred.put("ua", json!("Mozilla/5.0 Account/9.0"));
        let headers = p.headers(&cred);
        let ua = headers
            .iter()
            .find(|(key, _)| key == "User-Agent")
            .map(|(_, value)| value.as_str());
        assert_eq!(ua, Some("Mozilla/5.0 Account/9.0"));

        let old = Credential {
            token: "tk".into(),
            ..Default::default()
        };
        let headers = p.headers(&old);
        let ua = headers
            .iter()
            .find(|(key, _)| key == "User-Agent")
            .map(|(_, value)| value.as_str());
        assert_eq!(ua, Some(USER_AGENT), "旧凭据没有 ua 时回退常量");
    }

    #[test]
    fn tampered_field_is_an_error_not_a_panic() {
        let mut field = seal("x");
        let obj = field.as_object_mut().unwrap();
        obj.insert("envelope".into(), json!("bm90LWEtcmVhbC1lbnZlbG9wZQ=="));
        assert!(open_field(&field).is_err());
    }

    #[test]
    fn scan_reads_ticket_and_keeps_bytes() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("auth")).unwrap();
        let raw = ticket("u1", "tk1", "凌致");
        std::fs::write(dir.path().join("auth/workbuddy-desktop.info"), &raw).unwrap();

        let mut found = Vec::new();
        let count = provider(dir.path())
            .scan(&mut |acc| {
                found.push(acc);
                Ok(())
            })
            .unwrap();

        assert_eq!(count, 1);
        assert_eq!(found[0].identity.name, "凌致");
        assert_eq!(found[0].identity.iid, "workbuddy:u1");
        assert!(found[0].identity.requires_checkin);

        let cred = Credential::parse(&found[0].secret).unwrap();
        assert_eq!(cred.token, "tk1");
        assert_eq!(cred.meta_str("uid"), Some("u1"));
        let stored = base64::engine::general_purpose::STANDARD
            .decode(cred.meta_str("ticket").unwrap())
            .unwrap();
        assert_eq!(stored, raw, "契约 8a：票据必须按字节保存");
    }

    #[test]
    fn duplicate_backups_collapse_to_one_account() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("auth")).unwrap();
        std::fs::write(
            dir.path().join("auth/workbuddy-desktop.info"),
            ticket("u1", "new", "新"),
        )
        .unwrap();
        std::fs::write(
            dir.path()
                .join("auth/workbuddy-desktop.info.bak-20260101-000000"),
            ticket("u1", "old", "旧"),
        )
        .unwrap();

        let mut names = Vec::new();
        let count = provider(dir.path())
            .scan(&mut |acc| {
                names.push(acc.identity.name.clone());
                Ok(())
            })
            .unwrap();
        assert_eq!(count, 1, "同一账号的多份备份只能算一个");
        assert_eq!(names[0], "新", "应取最新的一份");
    }

    #[test]
    fn intl_ticket_is_not_required_to_checkin() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("auth")).unwrap();
        std::fs::write(
            dir.path().join("auth/workbuddy-desktop-ai.info"),
            ticket("u2", "tk", "bob"),
        )
        .unwrap();
        let intl = WorkBuddyProvider::at(
            true,
            dir.path().join("auth"),
            dir.path().join("client"),
            dir.path().join("profiles"),
        );
        let mut rows = Vec::new();
        intl.scan(&mut |acc| {
            rows.push(acc);
            Ok(())
        })
        .unwrap();
        assert_eq!(rows[0].identity.iid, "workbuddy:u2@intl");
        assert!(!rows[0].identity.requires_checkin, "国际版免签到");
    }

    #[test]
    fn missing_auth_dir_returns_zero() {
        let dir = tempfile::TempDir::new().unwrap();
        assert_eq!(provider(dir.path()).scan(&mut |_| Ok(())).unwrap(), 0);
    }

    /// Chromium 的纯缓存不参与登录判定，却会被整目录拷进每个账号档案，
    /// 再被退出登录备份、手工快照各复制一遍。先固定「档案里只留登录相关文件」。
    #[test]
    fn save_pack_does_not_archive_chromium_caches() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = provider(dir.path());
        let client = dir.path().join("client");
        let session = client.join("app/session");
        std::fs::create_dir_all(session.join("Cache")).unwrap();
        std::fs::create_dir_all(session.join("Code Cache")).unwrap();
        std::fs::create_dir_all(session.join("Local Storage")).unwrap();
        std::fs::write(session.join("Cache/data_1"), vec![0u8; 4096]).unwrap();
        std::fs::write(session.join("Code Cache/js"), b"compiled").unwrap();
        std::fs::write(session.join("Cookies"), b"session-cookie").unwrap();
        std::fs::write(session.join("Local Storage/leveldb"), b"tokens").unwrap();
        std::fs::create_dir_all(client.join("storage/skeleton")).unwrap();
        std::fs::write(
            client.join("storage/skeleton/account-snapshot.json"),
            b"{\"primary\":{}}",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("auth")).unwrap();
        std::fs::write(
            dir.path().join("auth/workbuddy-desktop.info"),
            ticket("u1", "tk", "n"),
        )
        .unwrap();

        p.save_pack("u1").unwrap();
        let pack = p.pack_dir("u1");
        assert!(
            !pack.join("app/session/Cache").exists(),
            "纯缓存不该进账号档案"
        );
        assert!(
            !pack.join("app/session/Code Cache").exists(),
            "纯缓存不该进账号档案"
        );
        assert_eq!(
            std::fs::read(pack.join("app/session/Cookies")).unwrap(),
            b"session-cookie"
        );
        assert_eq!(
            std::fs::read(pack.join("app/session/Local Storage/leveldb")).unwrap(),
            b"tokens"
        );
        assert!(p.pack_ready("u1"));
    }

    /// 精简档案（不含缓存）仍要能完整换回登录态，票据按字节写回。
    #[test]
    fn restore_from_slim_pack_swaps_login_state() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = provider(dir.path());
        let client = dir.path().join("client");
        let auth = dir.path().join("auth");
        std::fs::create_dir_all(&auth).unwrap();

        let pack = p.pack_dir("u1");
        std::fs::create_dir_all(pack.join("app/session/Local Storage")).unwrap();
        std::fs::create_dir_all(pack.join("storage/skeleton")).unwrap();
        std::fs::write(pack.join("app/session/Cookies"), b"B").unwrap();
        std::fs::write(pack.join("app/session/Local Storage/leveldb"), b"B-tokens").unwrap();
        std::fs::write(pack.join("storage/skeleton/account-snapshot.json"), b"{}").unwrap();
        let saved = ticket("u1", "tk", "n");
        std::fs::write(pack.join("auth.info"), &saved).unwrap();

        // 客户端当前是别的账号，还带着自己的缓存
        std::fs::create_dir_all(client.join("app/session/Cache")).unwrap();
        std::fs::write(client.join("app/session/Cache/data"), b"old").unwrap();
        std::fs::write(client.join("app/session/Cookies"), b"A").unwrap();
        std::fs::create_dir_all(client.join("storage/skeleton")).unwrap();
        std::fs::write(
            auth.join("workbuddy-desktop.info"),
            b"{\"account\":{\"uid\":\"u2\"}}",
        )
        .unwrap();

        p.restore_pack("u1").unwrap();
        assert_eq!(
            std::fs::read(auth.join("workbuddy-desktop.info")).unwrap(),
            saved,
            "契约 8a：票据按字节写回"
        );
        assert_eq!(
            std::fs::read(client.join("app/session/Cookies")).unwrap(),
            b"B"
        );
        assert_eq!(
            std::fs::read(client.join("app/session/Local Storage/leveldb")).unwrap(),
            b"B-tokens"
        );
        assert!(
            !client.join("app/session/Cache").exists(),
            "旧账号缓存不能串到新账号"
        );
    }

    /// 旧版 `save_pack` 在 rename 失败时会同时丢掉正式档案、只留下完整 tmp。
    /// 这些账号必须能被认领回来，否则永远报「本机没有完整登录态档案」。
    #[test]
    fn orphan_tmp_pack_is_claimed_back() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = provider(dir.path());
        let tmp = p.pack_dir("u1").with_extension("tmp");
        std::fs::create_dir_all(tmp.join("app/session")).unwrap();
        std::fs::write(tmp.join("auth.info"), ticket("u1", "tk", "n")).unwrap();

        assert!(
            p.pack_ready("u1"),
            "内容完整的 tmp 必须被认领，否则这个账号永远切不过去"
        );
        assert!(
            p.pack_dir("u1").join("app/session").is_dir(),
            "认领后应提升为正式目录"
        );
        assert!(!tmp.exists(), "提升成功后 tmp 不该留着");
    }

    #[test]
    fn half_written_tmp_pack_is_not_claimed() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = provider(dir.path());
        let tmp = p.pack_dir("u1").with_extension("tmp");
        std::fs::create_dir_all(tmp.join("app/session")).unwrap();
        // 缺 auth.info：切过去只会停在登录页，不能当成可用档案
        assert!(!p.pack_ready("u1"));
    }

    #[test]
    fn repeated_save_leaves_no_staging_dirs() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = provider(dir.path());
        let client = dir.path().join("client");
        std::fs::create_dir_all(client.join("app/session")).unwrap();
        std::fs::write(client.join("app/session/Cookies"), b"c").unwrap();
        std::fs::create_dir_all(dir.path().join("auth")).unwrap();
        std::fs::write(
            dir.path().join("auth/workbuddy-desktop.info"),
            ticket("u1", "tk", "n"),
        )
        .unwrap();

        p.save_pack("u1").unwrap();
        p.save_pack("u1").unwrap();

        assert!(p.pack_ready("u1"));
        assert!(!p.pack_dir("u1").with_extension("tmp").exists());
        assert!(
            !p.pack_dir("u1").with_extension("old").exists(),
            "换名用的旧目录不能残留"
        );
    }

    #[test]
    fn save_and_restore_pack_roundtrip() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = provider(dir.path());
        let client = dir.path().join("client");
        std::fs::create_dir_all(client.join("app/session/Local Storage")).unwrap();
        std::fs::create_dir_all(client.join("storage/skeleton")).unwrap();
        std::fs::write(client.join("app/session/Local Storage/leveldb"), b"cookies").unwrap();
        std::fs::write(
            client.join("storage/skeleton/account-snapshot.json"),
            b"{\"primary\":{}}",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("auth")).unwrap();
        std::fs::write(
            dir.path().join("auth/workbuddy-desktop.info"),
            ticket("u1", "tk", "n"),
        )
        .unwrap();

        p.save_pack("u1").unwrap();
        assert!(p.pack_ready("u1"));
        let saved_ticket = std::fs::read(p.pack_dir("u1").join("auth.info")).unwrap();

        // 客户端换成别人：登录态被清空、票据被改写
        fsx::remove_path(&client.join("app/session")).unwrap();
        std::fs::write(
            dir.path().join("auth/workbuddy-desktop.info"),
            b"{\"account\":{\"uid\":\"u2\"}}",
        )
        .unwrap();

        p.restore_pack("u1").unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("auth/workbuddy-desktop.info")).unwrap(),
            saved_ticket,
            "契约 8a：票据必须按字节写回"
        );
        assert_eq!(
            std::fs::read(client.join("app/session/Local Storage/leveldb")).unwrap(),
            b"cookies"
        );
    }

    /// 完整切换：当前登录 A，目标 B 已有档案 → 换过去、A 被自动存档。
    #[test]
    fn switch_swaps_packs_and_archives_current_account() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = provider(dir.path()).with_processes(&[]); // 本机可能正开着客户端
        let client = dir.path().join("client");
        let auth = dir.path().join("auth");
        std::fs::create_dir_all(client.join("app/session")).unwrap();
        std::fs::create_dir_all(client.join("storage/skeleton")).unwrap();
        std::fs::create_dir_all(&auth).unwrap();

        // 当前登录 A
        std::fs::write(client.join("app/session/cookies"), b"A-cookies").unwrap();
        std::fs::write(
            client.join("storage/skeleton/account-snapshot.json"),
            b"{\"primary\":{\"uid\":\"u1\"}}",
        )
        .unwrap();
        std::fs::write(
            auth.join("workbuddy-desktop.info"),
            ticket("u1", "tk-a", "A"),
        )
        .unwrap();

        // B 的档案（登录态 + 票据）
        let pack_b = p.pack_dir("u2");
        std::fs::create_dir_all(pack_b.join("app/session")).unwrap();
        std::fs::create_dir_all(pack_b.join("storage/skeleton")).unwrap();
        std::fs::write(pack_b.join("app/session/cookies"), b"B-cookies").unwrap();
        std::fs::write(
            pack_b.join("storage/skeleton/account-snapshot.json"),
            b"{\"primary\":{\"uid\":\"u2\"}}",
        )
        .unwrap();
        let ticket_b = ticket("u2", "tk-b", "B");
        std::fs::write(pack_b.join("auth.info"), &ticket_b).unwrap();

        let out = switch_to(&p, "u2", "B").unwrap();

        assert_eq!(
            p.current_uid().as_deref(),
            Some("u2"),
            "切换后客户端应登录 B"
        );
        assert_eq!(
            std::fs::read(auth.join("workbuddy-desktop.info")).unwrap(),
            ticket_b,
            "契约 8a：票据按字节写回"
        );
        assert_eq!(
            std::fs::read(client.join("app/session/cookies")).unwrap(),
            b"B-cookies"
        );
        assert!(p.pack_ready("u1"), "当前账号必须先被存档，否则切不回去");
        assert!(out.message.contains("存档"));
    }

    #[test]
    fn travel_status_normalizes_the_real_arrived_shape() {
        let data = json!({
            "state": "arrived",
            "buddy_id": 12314099,
            "record_id": 38593793,
            "location": {"id": 1, "code": "coffee", "name": "咖啡馆", "duration_hours": 3},
            "depart_at": 1793519788,
            "arrive_at": 1793530588,
            "server_now": 1793870943,
            "letter": {"id": 2, "text": "亲爱的铲屎官：……", "guide_text": "看看工具"},
            "daily_limit_reached": false,
            "duration_hours": 3,
            "reward_credit": 8
        });
        let status = normalize_travel_status(&data);
        assert_eq!(status.state, Some(TravelState::Arrived));
        assert_eq!(
            status.location.as_ref().map(|l| l.name.as_str()),
            Some("咖啡馆")
        );
        assert_eq!(
            status.arrive_at,
            Some(1_793_530_588_000),
            "秒级时间要转毫秒"
        );
        assert_eq!(status.reward_credit, Some(8.0));
        assert_eq!(status.record_id, Some(38_593_793));
        assert!(!status.daily_limit_reached);
        assert!(status.letter.unwrap().contains("铲屎官"));
    }

    #[test]
    fn travel_limit_wins_over_idle() {
        let data = json!({"state": "idle", "daily_limit_reached": true});
        assert_eq!(
            normalize_travel_status(&data).state,
            Some(TravelState::Limit),
            "额度用完不能当成可派出"
        );
        let data = json!({"state": "traveling", "arrive_at": 0});
        let status = normalize_travel_status(&data);
        assert_eq!(status.state, Some(TravelState::Traveling));
        assert_eq!(status.arrive_at, None, "0 表示没有时间，不能显示成 1970 年");
    }

    #[test]
    fn travel_location_and_record_parse_the_real_fields() {
        let location = travel_location(&json!({
            "id": 1, "code": "coffee", "name": "咖啡馆",
            "duration_hours_min": 1, "duration_hours_max": 4,
            "reward_credit_min": 5, "reward_credit_max": 10
        }))
        .unwrap();
        assert_eq!(location.name, "咖啡馆");
        assert_eq!(location.duration_hours_max, Some(4));
        assert_eq!(location.reward_credit_min, Some(5.0));

        let record = normalize_travel_record(&json!({
            "record_id": 34232265,
            "location": {"id": 1, "code": "coffee", "name": "咖啡馆"},
            "depart_at": 1793234984,
            "arrive_at": 1793242184,
            "claimed_at": 1793519786,
            "reward_credit": 7,
            "travel_date": "2026-10-29",
            "letter": {"text": "记录信件"}
        }));
        assert_eq!(record.location_name.as_deref(), Some("咖啡馆"));
        assert_eq!(record.reward_credit, Some(7.0));
        assert_eq!(record.travel_date.as_deref(), Some("2026-10-29"));
        assert!(record.claimed_at.is_some());
    }

    #[tokio::test]
    async fn travel_claim_and_depart_hit_the_expected_paths() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = provider(dir.path());
        let mut cred = Credential {
            token: "tk".into(),
            ..Default::default()
        };
        cred.put("uid", json!("u1"));

        let http = MockHttp::new();
        let claim_url = format!("{TRAVEL_BASE}{TRAVEL_CLAIM}");
        http.on(
            &claim_url,
            200,
            r#"{"code":0,"msg":"OK","data":{"reward_credit":8}}"#,
        );
        let claimed = p.travel_claim(&cred, &http).await.unwrap();
        assert!(claimed.ok);
        assert_eq!(claimed.granted, Some(8.0));
        assert_eq!(http.call_count(&claim_url), 1);

        let depart_url = format!("{TRAVEL_BASE}{TRAVEL_DEPART}");
        http.on(
            &depart_url,
            200,
            r#"{"code":0,"msg":"OK","data":{"location":{"id":1,"name":"咖啡馆"},"arrive_at":1793530588}}"#,
        );
        let departed = p.travel_depart(&cred, &http, 1).await.unwrap();
        assert!(departed.ok);
        assert!(departed.message.contains("咖啡馆"));
        assert_eq!(http.call_count(&depart_url), 1);
    }

    #[tokio::test]
    async fn travel_reward_does_not_complete_daily_checkin() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = provider(dir.path());
        let mut cred = Credential {
            token: "tk".into(),
            ..Default::default()
        };
        cred.put("uid", json!("u1"));
        let http = MockHttp::new();
        http.on(
            &format!("{TRAVEL_BASE}{TRAVEL_CLAIM}"),
            200,
            r#"{"code":0,"data":{"reward_credit":8}}"#,
        );
        http.on(
            &format!("{}/checkin-activity-status", p.api_base()),
            200,
            r#"{"code":0,"data":{"active":true,"today_checked_in":false}}"#,
        );
        http.on(&format!("{}/get-user-resource", p.api_base()), 200,
            r#"{"code":0,"data":{"Data":{"Accounts":[{"CycleCapacityRemain":108,"PackageName":"旅行奖励"}]}}}"#);
        let daily_url = format!("{}/daily-checkin", p.api_base());
        http.on(&daily_url, 200, r#"{"code":0,"data":{"credit":10}}"#);
        assert_eq!(
            p.travel_claim(&cred, &http).await.unwrap().granted,
            Some(8.0)
        );
        let status = p.status(&cred, &http).await.unwrap();
        assert_eq!(status.checked_in, Some(false));
        assert_eq!(status.balance, Some(108.0));
        assert_eq!(http.call_count(&daily_url), 0);
        let result = p.checkin(&cred, &http).await.unwrap();
        assert!(result.ok && !result.already);
        assert_eq!(result.granted, Some(10.0));
        assert_eq!(http.call_count(&daily_url), 1);
    }

    #[test]
    fn intl_has_no_travel_activity() {
        let dir = tempfile::TempDir::new().unwrap();
        let intl = WorkBuddyProvider::at(
            true,
            dir.path().join("auth"),
            dir.path().join("client"),
            dir.path().join("profiles"),
        );
        assert!(!intl.travel_supported(), "国际版没有猫猫旅行");
        assert!(provider(dir.path()).travel_supported(), "国内版应支持");
    }

    #[test]
    fn switch_without_pack_touches_nothing() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = provider(dir.path());
        std::fs::create_dir_all(dir.path().join("auth")).unwrap();
        std::fs::write(
            dir.path().join("auth/workbuddy-desktop.info"),
            ticket("u1", "tk", "n"),
        )
        .unwrap();
        let before = std::fs::read(dir.path().join("auth/workbuddy-desktop.info")).unwrap();

        let err = switch_to(&p, "u9", "目标").unwrap_err();
        assert!(matches!(
            err.kind,
            crate::app::error::ErrorKind::SessionStateMissing
        ));
        assert_eq!(
            std::fs::read(dir.path().join("auth/workbuddy-desktop.info")).unwrap(),
            before,
            "没有档案时一个字节都不能写"
        );
    }
}
