use super::*;

fn setup() -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open_at(&dir.path().join("catalog.db")).unwrap();
    db.write(bootstrap).unwrap();
    (dir, db)
}

fn sample(source: &str, id: &str) -> MediaDraft {
    MediaDraft {
        source: source.into(),
        external_id: id.into(),
        media: CatalogMedia {
            title: "测试番".into(),
            kind: "anime".into(),
            regions: vec!["JP".into()],
            genres: vec!["Drama".into()],
            platforms: vec!["bilibili".into()],
            status: "airing".into(),
            year: Some(2026),
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn display_title_prefers_short_chinese_name() {
    assert_eq!(
        choose_display_title("The Legend", &["传说".into(), "很长的中文别名也要靠后".into()]),
        "传说"
    );
    assert_eq!(choose_display_title("已经中文", &["Another Name".into()]), "已经中文");
    assert_eq!(choose_display_title("Solo Leveling", &["我独自升级".into()]), "我独自升级");
    assert_eq!(choose_display_title("Only English", &[]), "Only English");
}

fn episode(id: &str, date: &str) -> CatalogRelease {
    CatalogRelease {
        source: "bilibili".into(),
        external_id: id.into(),
        date: date.into(),
        local_time: Some("23:30".into()),
        timezone: Some("Asia/Shanghai".into()),
        utc_time: Some(format!("{date}T15:30:00Z")),
        season: Some(1),
        episode: Some(id.into()),
        release_kind: "episode".into(),
        ..Default::default()
    }
}

fn rating(source: &str, category: &str, value: f64, scale: f64) -> CatalogRating {
    CatalogRating {
        source: source.into(),
        category: category.into(),
        value: Some(value),
        scale,
        votes: Some(123),
        reviews: Some(12),
        positive_percent: Some(88.0),
        rank: Some(7),
        url: Some(format!("https://example.test/{source}")),
        ..Default::default()
    }
}

#[test]
fn two_episode_dates_survive_and_unchanged_resync_is_deduplicated() {
    let (_dir, db) = setup();
    let mut draft = sample("bilibili", "season-1");
    draft.releases = vec![episode("ep1", "2026-11-18"), episode("ep2", "2026-11-22")];
    let counts = upsert(&db, &[draft.clone()], "2026-11-22T01:00:00Z").unwrap();
    assert_eq!((counts.added, counts.releases), (1, 2));
    let counts = upsert(&db, &[draft.clone()], "2026-11-23T01:00:00Z").unwrap();
    assert_eq!(
        (
            counts.added,
            counts.updated,
            counts.deduplicated,
            counts.releases
        ),
        (0, 0, 1, 0)
    );
    let page = releases(
        &db,
        &CatalogQuery {
            start: Some("2026-11-18".into()),
            end: Some("2026-11-23".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(page.total, 2);
    assert_eq!(page.items[0].release.date, "2026-11-18");
    assert_eq!(page.items[1].release.date, "2026-11-22");
    assert_eq!(
        page.items[1].release.timezone.as_deref(),
        Some("Asia/Shanghai")
    );
    assert_eq!(page.items[1].release.local_time.as_deref(), Some("23:30"));
    assert_eq!(
        page.items[1].release.utc_time.as_deref(),
        Some("2026-11-22T15:30:00Z")
    );
    draft.releases[0].date = "2026-11-19".into();
    let counts = upsert(&db, &[draft], "2026-11-24T01:00:00Z").unwrap();
    assert_eq!((counts.updated, counts.releases), (1, 1));
    let detail = detail(&db, "bilibili:season-1").unwrap().unwrap();
    assert_eq!(detail.releases.len(), 2);
    assert!(detail
        .releases
        .iter()
        .any(|release| release.date == "2026-11-19"));
    assert_eq!(detail.media.first_seen, "2026-11-22T01:00:00Z");
    db.read(|conn| {
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM episodes", [], |row| row.get(0))?;
        assert_eq!(count, 2);
        Ok(())
    })
    .unwrap();
}

#[test]
fn cross_source_merge_by_identity_title_and_year_stays() {
    let (_dir, db) = setup();
    let mut first = sample("anilist", "10");
    first.external_ids.push(CatalogExternalId {
        source: "mal".into(),
        external_id: "42".into(),
    });
    upsert(&db, &[first], "2026-11-20").unwrap();
    let mut second = sample("jikan", "42");
    second.external_ids.push(CatalogExternalId {
        source: "mal".into(),
        external_id: "42".into(),
    });
    second.media.cast = vec!["声優甲".into()];
    let counts = upsert(&db, &[second], "2026-11-21").unwrap();
    assert_eq!((counts.added, counts.updated), (0, 1));
    assert_eq!(count_all(&db).unwrap(), 1);
    assert_eq!(count_source(&db, "jikan").unwrap(), 1);
    let item = detail(&db, "anilist:10").unwrap().unwrap();
    assert_eq!(item.media.source, "anilist");
    assert_eq!(item.external_ids.len(), 3);
    assert_eq!(item.media.cast, vec!["声優甲"]);
    // 新语义：各来源落地信息在详情逐源保留
    assert!(
        item.source_infos.iter().any(|s| s.source == "jikan"),
        "合并后的行要有 jikan 的来源信息"
    );

    // 同标题 + 同类型 + 同年份（不同季的外部 ID）→ 合并进同一作品，季各自保留
    let mut other_season = sample("anilist", "12");
    other_season.seasons.push(CatalogSeason {
        source: "anilist".into(),
        external_id: "s2".into(),
        number: Some(2),
        ..Default::default()
    });
    upsert(&db, &[other_season], "2026-11-22").unwrap();
    assert_eq!(count_all(&db).unwrap(), 1);
    let merged = detail(&db, "anilist:10").unwrap().unwrap();
    assert_eq!(merged.external_ids.len(), 4);
    assert_eq!(merged.seasons.len(), 1, "第二季挂在同一作品下");
    assert_eq!(merged.seasons[0].external_id, "s2");

    // 同标题但年份冲突 → 不自动合并，自成一行并记入疑似候选
    let mut other_year = sample("anilist", "11");
    other_year.media.year = Some(2025);
    upsert(&db, &[other_year], "2026-11-23").unwrap();
    assert_eq!(count_all(&db).unwrap(), 2);
    let with_suspect = detail(&db, "anilist:10").unwrap().unwrap();
    assert_eq!(with_suspect.dupe_candidates.len(), 1);
    assert_eq!(with_suspect.dupe_candidates[0].reason, "year_conflict");
    db.read(|conn| {
        let status: String = conn
            .query_row(
                "SELECT status FROM media_dupe_candidates WHERE key_a='anilist:10' AND key_b='anilist:11'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "open");
        Ok(())
    })
    .unwrap();

    // 人工合并：确认是同一作品的另一季/另一来源写法
    let message = merge_candidates(&db, "anilist:10", "anilist:11", true, "2026-11-24").unwrap();
    assert!(message.contains("已合并"));
    assert_eq!(count_all(&db).unwrap(), 1);
    let after = detail(&db, "anilist:10").unwrap().unwrap();
    assert!(after.external_ids.iter().any(|id| id.source == "anilist" && id.external_id == "11"));
    db.read(|conn| {
        let archived: i64 = conn
            .query_row("SELECT COUNT(*) FROM media_merge_archive WHERE merged_key='anilist:11'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(archived, 1, "被合并的行要归档可追溯");
        Ok(())
    })
    .unwrap();
}

#[test]
fn migration_preserves_legacy_favorites_dates_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open_at(&dir.path().join("legacy.db")).unwrap();
    db.write(|conn| {
        // Db::open_at may already bootstrap the new DAL; this is an isolated fixture.
        conn.execute("DELETE FROM catalog_meta WHERE key = ?1", [LEGACY_MARKER])?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS catalog_items (
            key TEXT PRIMARY KEY, platform TEXT NOT NULL, kind TEXT NOT NULL,
            title TEXT NOT NULL, update_note TEXT, year INTEGER, air_date TEXT,
            finished INTEGER NOT NULL DEFAULT 0, url TEXT, first_seen TEXT NOT NULL,
            updated_at TEXT NOT NULL, favorite INTEGER NOT NULL DEFAULT 0
        );
        INSERT INTO catalog_items VALUES ('tencent:old','tencent','tv','旧剧','更新至8集',2026,
            '2026-11-17',0,'https://example.test/tv','2026-11-01','2026-11-19',1);
        INSERT INTO catalog_items VALUES ('iqiyi:old','iqiyi','tv','爱奇艺旧剧',NULL,2026,
            '2026-11-16',0,NULL,'2026-11-02','2026-11-18',1);
        INSERT INTO catalog_items VALUES ('bilibili:old','bilibili','anime','旧番','第2话',2026,
            '2026-11-18',0,NULL,'2026-11-02','2026-11-20',1);",
        )?;
        bootstrap(conn)?;
        bootstrap(conn)?;
        let old_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM catalog_items WHERE favorite=1",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(old_count, 3);
        Ok(())
    })
    .unwrap();
    assert_eq!(count_all(&db).unwrap(), 3);
    let item = detail(&db, "tencent:old").unwrap().unwrap();
    assert_eq!(item.media.premiered.as_deref(), Some("2026-11-17"));
    assert_eq!(item.media.first_seen, "2026-11-01");
    assert_eq!(item.media.updated_at, "2026-11-19");
    assert!(item.releases.is_empty());
    assert!(detail(&db, "iqiyi:old")
        .unwrap()
        .unwrap()
        .releases
        .is_empty());
    let item = detail(&db, "bilibili:old").unwrap().unwrap();
    assert_eq!(item.releases.len(), 1);
    assert_eq!(item.releases[0].release_kind, "legacy");
    assert_eq!(item.releases[0].source, "bilibili");
    assert_eq!(item.media.premiered, None);
}

#[test]
fn literal_multilingual_search_and_combined_filters_paginate_on_server() {
    let (_dir, db) = setup();
    let mut a = sample("anilist", "1");
    a.media.aliases = vec!["別名百分%_\\百".into(), "Romanized title".into()];
    a.media.english_title = Some("English name".into());
    a.media.cast = vec!["声優日本語".into()];
    a.media.directors = vec!["导演甲".into()];
    a.media.studios = vec!["Studio Z".into()];
    a.ratings = vec![rating("anilist", "user", 80.0, 100.0)];
    let mut b = a.clone();
    b.external_id = "2".into();
    b.media.title = "第二部".into();
    let mut c = sample("anilist", "3");
    c.media.year = Some(2025);
    upsert(&db, &[a, b, c], "2026-11-22").unwrap();
    for text in [
        "別名百分%_\\百",
        "Romanized",
        "English",
        "声優日本語",
        "导演甲",
        "Studio Z",
        "%_",
    ] {
        assert_eq!(
            library(
                &db,
                &CatalogQuery {
                    search: Some(text.into()),
                    ..Default::default()
                }
            )
            .unwrap()
            .total,
            2,
            "{text}"
        );
    }
    let page = library(
        &db,
        &CatalogQuery {
            search: Some("声優日本語".into()),
            kind: Some("anime".into()),
            region: Some("JP".into()),
            genre: Some("剧情".into()),
            platform: Some("bilibili".into()),
            status: Some("airing".into()),
            year: Some(2026),
            min_rating: Some(7.5),
            page: Some(2),
            page_size: Some(1),
            sort: Some("title".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        (page.total, page.page, page.page_size, page.items.len()),
        (2, 2, 1, 1)
    );
    assert_eq!(
        library(
            &db,
            &CatalogQuery {
                search: Some("' OR 1=1 --".into()),
                ..Default::default()
            }
        )
        .unwrap()
        .total,
        0
    );
    assert_eq!(
        library(
            &db,
            &CatalogQuery {
                page: Some(0),
                page_size: Some(1000),
                ..Default::default()
            }
        )
        .unwrap()
        .page_size,
        100
    );
    let facets = facets(&db, &CatalogQuery::default()).unwrap();
    assert_eq!(facets.kinds[0].label, "动漫");
    assert_eq!(facets.regions[0].label, "日本");
    assert_eq!(facets.genres[0].count, 3);
}

#[test]
fn facets_follow_other_filters_but_not_own_group() {
    let (_dir, db) = setup();
    let mut anime = sample("tmdb", "anime");
    anime.releases = vec![episode("anime-1", "2026-11-18")];
    let mut tv = sample("tmdb", "tv");
    tv.media.title = "另一部".into();
    tv.media.kind = "tv".into();
    tv.media.regions = vec!["US".into()];
    tv.media.genres = vec!["Comedy".into()];
    tv.media.platforms = vec!["netflix".into()];
    tv.media.year = Some(2025);
    tv.releases = vec![episode("tv-1", "2026-11-22")];
    upsert(&db, &[anime, tv], "2026-11-22").unwrap();

    let count = |list: &[CatalogFacetCount], key: &str| {
        list.iter().find(|item| item.key == key).map_or(0, |item| item.count)
    };

    let jp = facets(&db, &CatalogQuery { region: Some("JP".into()), ..Default::default() }).unwrap();
    assert_eq!(count(&jp.kinds, "anime"), 1);
    assert_eq!(count(&jp.kinds, "tv"), 0);
    // 本组过滤不参与本组计数，换其他选项的后果才看得见
    assert_eq!(count(&jp.regions, "JP"), 1);
    assert_eq!(count(&jp.regions, "US"), 1);
    assert_eq!(count(&jp.genres, "剧情"), 1);
    assert_eq!(count(&jp.genres, "喜剧"), 0);
    assert_eq!(jp.years, vec![2026]);

    let anime_only = facets(&db, &CatalogQuery { kind: Some("anime".into()), ..Default::default() }).unwrap();
    assert_eq!(count(&anime_only.kinds, "tv"), 1);
    assert_eq!(count(&anime_only.regions, "US"), 0);

    let year = facets(&db, &CatalogQuery { year: Some(2026), ..Default::default() }).unwrap();
    assert!(year.years.contains(&2025));

    let week = facets(&db, &CatalogQuery {
        start: Some("2026-11-22".into()),
        end: Some("2026-11-23".into()),
        ..Default::default()
    }).unwrap();
    assert_eq!(count(&week.kinds, "tv"), 1);
    assert_eq!(count(&week.kinds, "anime"), 0);
}

#[test]
fn original_rating_sources_and_categories_remain_separate() {
    let (_dir, db) = setup();
    let mut draft = sample("tmdb", "show/1");
    draft.ratings = vec![
        rating("tmdb", "user", 8.0, 10.0),
        rating("rt", "audience", 70.0, 100.0),
        rating("rt", "critic", 99.0, 100.0),
    ];
    upsert(&db, &[draft.clone()], "2026-11-22").unwrap();
    let item = detail(&db, "tmdb:show/1").unwrap().unwrap();
    assert_eq!(item.ratings.len(), 3);
    assert_eq!(item.media.rating, Some(8.0));
    assert_eq!(item.media.rating_source.as_deref(), Some("tmdb"));
    assert_eq!(
        library(
            &db,
            &CatalogQuery {
                min_rating: Some(8.5),
                ..Default::default()
            }
        )
        .unwrap()
        .total,
        0
    );
    let rt = item
        .ratings
        .iter()
        .find(|r| r.source == "rt" && r.category == "audience")
        .unwrap();
    assert_eq!(
        (
            rt.value,
            rt.scale,
            rt.votes,
            rt.reviews,
            rt.rank,
            rt.positive_percent
        ),
        (Some(70.0), 100.0, Some(123), Some(12), Some(7), Some(88.0))
    );
    assert_eq!(upsert(&db, &[draft], "2026-11-23").unwrap().deduplicated, 1);
    db.read(|conn| {
        let count: i64 =
            conn.query_row("SELECT COUNT(*) FROM media_reviews", [], |row| row.get(0))?;
        assert_eq!(count, 3);
        Ok(())
    })
    .unwrap();
}

#[test]
fn display_rating_keeps_original_scale_and_chapters_are_individual_facts() {
    let (_dir, db) = setup();
    let mut draft = sample("anilist", "manga-1");
    draft.media.kind = "manga".into();
    draft.ratings = vec![rating("anilist", "user", 80.0, 100.0)];
    draft.releases = vec![
        CatalogRelease {
            source: "anilist".into(),
            external_id: "chapter-1".into(),
            date: "2026-11-22".into(),
            episode: Some("1".into()),
            release_kind: "chapter".into(),
            ..Default::default()
        },
        CatalogRelease {
            source: "anilist".into(),
            external_id: "chapter-2".into(),
            date: "2026-11-23".into(),
            episode: Some("2".into()),
            release_kind: "chapter".into(),
            ..Default::default()
        },
    ];
    upsert(&db, &[draft], "2026-11-22").unwrap();
    let item = detail(&db, "anilist:manga-1").unwrap().unwrap();
    assert_eq!(item.media.rating, Some(80.0));
    assert_eq!(item.ratings[0].scale, 100.0);
    assert_eq!(item.releases.len(), 2);
    assert_eq!(
        library(
            &db,
            &CatalogQuery {
                min_rating: Some(8.5),
                ..Default::default()
            }
        )
        .unwrap()
        .total,
        0
    );
    assert_eq!(
        library(
            &db,
            &CatalogQuery {
                min_rating: Some(8.0),
                ..Default::default()
            }
        )
        .unwrap()
        .total,
        1
    );
    db.read(|conn| {
        let count: i64 =
            conn.query_row("SELECT COUNT(*) FROM manga_chapters", [], |row| row.get(0))?;
        assert_eq!(count, 2);
        Ok(())
    })
    .unwrap();
}

#[test]
fn conflicting_trusted_ids_fail_atomically_without_erasing_either_item() {
    let (_dir, db) = setup();
    // 两条不同年份的样本：归一化标题相同但年份冲突，在新的二级匹配下仍各自成行，
    // 才能构造出「桥接条目同时指向两条不同行」的真实冲突场景。
    let mut older = sample("mal", "2");
    older.media.year = Some(2025);
    upsert(&db, &[sample("anilist", "1"), older], "2026-11-22").unwrap();
    let mut bridge = sample("tmdb", "3");
    bridge.external_ids = vec![
        CatalogExternalId {
            source: "anilist".into(),
            external_id: "1".into(),
        },
        CatalogExternalId {
            source: "mal".into(),
            external_id: "2".into(),
        },
    ];
    let error = upsert(&db, &[sample("tmdb", "new"), bridge], "2026-11-23").unwrap_err();
    assert_eq!(error.kind, ErrorKind::Conflict);
    assert_eq!(count_all(&db).unwrap(), 2);
    assert_eq!(count_source(&db, "tmdb").unwrap(), 0);
}

#[test]
fn all_sixteen_entities_and_foreign_keys_exist() {
    let (_dir, db) = setup();
    db.read(|conn| {
        for table in [
            "media",
            "media_aliases",
            "media_external_ids",
            "media_genres",
            "media_regions",
            "media_platforms",
            "seasons",
            "episodes",
            "manga_chapters",
            "release_schedules",
            "rating_sources",
            "media_ratings",
            "media_reviews",
            "data_sources",
            "ingestion_jobs",
            "ingestion_logs",
        ] {
            let exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                [table],
                |row| row.get(0),
            )?;
            assert!(exists, "{table}");
        }
        let mut stmt = conn.prepare("PRAGMA foreign_key_check")?;
        assert!(stmt.query([])?.next()?.is_none());
        Ok(())
    })
    .unwrap();
}

#[test]
fn source_keys_states_and_job_progress_do_not_overwrite_settings() {
    let (_dir, db) = setup();
    ensure_source(&db, "tmdb", true).unwrap();
    set_source_enabled(&db, "tmdb", false).unwrap();
    ensure_source(&db, "tmdb", true).unwrap();
    assert!(!source_enabled(&db, "tmdb").unwrap());
    set_source_key(&db, "tmdb", Some(&[1, 2, 3])).unwrap();
    let state = CatalogSourceStatus {
        source: "tmdb".into(),
        enabled: true,
        status: "ready".into(),
        ..Default::default()
    };
    set_source_state(&db, &state).unwrap();
    assert!(!source_state(&db, "tmdb").unwrap().unwrap().enabled);
    assert_eq!(source_key(&db, "tmdb").unwrap(), Some(vec![1, 2, 3]));
    let mut job = CatalogJob {
        id: "job-1".into(),
        source: "tmdb".into(),
        mode: "full".into(),
        started_at: "2026-11-22".into(),
        status: "running".into(),
        pages: 1,
        ..Default::default()
    };
    save_job(&db, &job).unwrap();
    save_job(&db, &job).unwrap();
    job.pages = 2;
    job.status = "failed".into();
    job.error = Some("token=secret-value raw response".into());
    save_job(&db, &job).unwrap();
    assert_eq!(jobs(&db).unwrap().len(), 1);
    db.read(|conn| {
        let logs: i64 =
            conn.query_row("SELECT COUNT(*) FROM ingestion_logs", [], |row| row.get(0))?;
        assert_eq!(logs, 2);
        let payload: String = conn.query_row(
            "SELECT payload FROM ingestion_jobs WHERE id='job-1'",
            [],
            |row| row.get(0),
        )?;
        assert!(!payload.contains("secret-value"));
        Ok(())
    })
    .unwrap();
}

#[test]
fn diagnostics_keep_real_errors_and_stay_none_on_success() {
    let (_dir, db) = setup();
    ensure_source(&db, "anilist", true).unwrap();
    let ok = CatalogSourceStatus { source: "anilist".into(), status: "success".into(), ..Default::default() };
    set_source_state(&db, &ok).unwrap();
    let stored = source_state(&db, "anilist").unwrap().unwrap();
    assert!(stored.error.is_none() && stored.warning.is_none(), "成功不得凭空生成错误/警告");
    let bad = CatalogSourceStatus {
        source: "anilist".into(), status: "failed".into(),
        error: Some("AniList 返回资料缺少 isAdult 字段".into()),
        warning: Some("分页预算已用完".into()), ..Default::default()
    };
    set_source_state(&db, &bad).unwrap();
    let stored = source_state(&db, "anilist").unwrap().unwrap();
    assert_eq!(stored.error.as_deref(), Some("AniList 返回资料缺少 isAdult 字段"));
    assert_eq!(stored.warning.as_deref(), Some("分页预算已用完"));
}

#[test]
fn specific_kind_is_not_downgraded_by_generic_tv_on_merge() {
    let (_dir, db) = setup();
    let mut variety = sample("tvmaze", "100");
    variety.media.kind = "variety".into();
    variety.media.updated_at = "2026-11-20T00:00:00+08:00".into();
    variety.external_ids = vec![CatalogExternalId { source: "imdb".into(), external_id: "tt1".into() }];
    upsert(&db, &[variety], "2026-11-20T00:00:00+08:00").unwrap();
    let mut generic = sample("simkl", "200");
    generic.media.kind = "tv".into();
    generic.media.updated_at = "2026-11-25T00:00:00+08:00".into();
    generic.external_ids = vec![CatalogExternalId { source: "imdb".into(), external_id: "tt1".into() }];
    upsert(&db, &[generic], "2026-11-25T00:00:00+08:00").unwrap();
    let merged = detail(&db, "tvmaze:100").unwrap().unwrap();
    assert_eq!(merged.media.kind, "variety", "通用 tv 不得把已判定的综艺降级");
    assert_eq!(count_all(&db).unwrap(), 1, "共享 IMDb ID 应合并为一条");
}

#[test]
fn overview_uses_real_release_and_first_seen_counts() {
    let (_dir, db) = setup();
    let mut draft = sample("bilibili", "1");
    draft.releases = vec![episode("ep1", "2026-11-22"), episode("ep2", "2026-11-23")];
    upsert(&db, &[draft], "2026-11-22T12:00:00Z").unwrap();
    let mut future = sample("tmdb", "2");
    future.media.kind = "tv".into();
    future.media.premiered = Some("2026-12-01".into());
    upsert(&db, &[future], "2026-11-20T12:00:00Z").unwrap();
    let overview = overview(&db, "2026-11-22", "2026-11-16", "2026-11-24", None).unwrap();
    assert_eq!(
        (
            overview.total,
            overview.today_added,
            overview.week_added,
            overview.today_releases,
            overview.week_updated
        ),
        (2, 1, 2, 1, 1)
    );
    assert_eq!(overview.upcoming.len(), 2);
    let page = releases(
        &db,
        &CatalogQuery {
            start: Some("2026-11-22".into()),
            end: Some("2026-11-23".into()),
            weekday: Some(7),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(page.total, 1);
}

#[test]
fn stale_metadata_cannot_erase_nonempty_fields_and_batch_errors_roll_back() {
    let (_dir, db) = setup();
    let mut draft = sample("anilist", "1");
    draft.media.updated_at = "2026-11-22".into();
    draft.media.premiered = Some("2026-11-22".into());
    draft.media.summary = Some("简介".into());
    upsert(&db, &[draft], "2026-11-22").unwrap();
    let mut stale = sample("anilist", "1");
    stale.media.updated_at = "2026-11-20".into();
    stale.media.status = "announced".into();
    stale.media.premiered = Some("2026-11-01".into());
    upsert(&db, &[stale], "2026-11-23").unwrap();
    let item = detail(&db, "anilist:1").unwrap().unwrap().media;
    assert_eq!(item.premiered.as_deref(), Some("2026-11-22"));
    assert_eq!(item.summary.as_deref(), Some("简介"));
    assert_eq!(item.status, "airing");
    let mut invalid = sample("anilist", "3");
    invalid.external_id.clear();
    assert!(upsert(&db, &[sample("anilist", "2"), invalid], "2026-11-24").is_err());
    assert_eq!(count_all(&db).unwrap(), 1);
}

#[test]
fn normalize_title_folds_width_punct_and_case() {
    assert_eq!(normalize_title("Ｄｒａｍａｔｉｃ 兴奋！"), "dramatic兴奋");
    assert_eq!(normalize_title("斗罗大陆 第一季"), normalize_title("斗罗大陆·第一季"));
    assert_eq!(normalize_title("  Re:Zero "), "rezero");
}

#[test]
fn scope_year_people_and_genre_filters_shape_the_library() {
    let (_dir, db) = setup();
    let mut domestic = sample("bilibili", "1");
    domestic.media.title = "国内剧".into();
    domestic.media.cast = vec!["演员甲".into(), "演员乙".into()];
    domestic.media.year = Some(2024);
    let mut foreign = sample("tvmaze", "2");
    foreign.media.title = "海外剧".into();
    foreign.media.cast = vec!["演员甲".into()];
    foreign.media.year = Some(2025);
    foreign.media.genres = vec!["科幻".into(), "悬疑".into()];
    upsert(&db, &[domestic], "2026-11-25").unwrap();
    upsert(&db, &[foreign], "2026-11-26").unwrap();

    let count = |query: CatalogQuery| -> i64 { library(&db, &query).unwrap().total };
    let all = CatalogQuery::default();
    assert_eq!(count(all.clone()), 2);
    assert_eq!(count(CatalogQuery { scope: Some("domestic".into()), ..Default::default() }), 1);
    assert_eq!(count(CatalogQuery { scope: Some("foreign".into()), ..Default::default() }), 1);
    // 演员任一（any）vs 全部（all）：「两个演员同框」只有国内剧满足
    assert_eq!(count(CatalogQuery { cast: vec!["演员甲".into()], ..Default::default() }), 2);
    assert_eq!(count(CatalogQuery {
        cast: vec!["演员甲".into(), "演员乙".into()],
        cast_mode: Some("all".into()),
        ..Default::default()
    }), 1);
    // 题材全部（all）：同时含科幻与悬疑
    assert_eq!(count(CatalogQuery {
        genres: vec!["科幻".into(), "悬疑".into()],
        genre_mode: Some("all".into()),
        ..Default::default()
    }), 1);
    // 年份区间
    assert_eq!(count(CatalogQuery { year_from: Some(2025), year_to: Some(2026), ..Default::default() }), 1);
    // updated 排序：最近更新的海外剧排最前
    let page = library(&db, &CatalogQuery { sort: Some("updated".into()), ..Default::default() }).unwrap();
    assert_eq!(page.items[0].title, "海外剧");
    // 组合：国内 + 2024 年区间 → 只剩国内剧
    assert_eq!(count(CatalogQuery {
        scope: Some("domestic".into()),
        year_from: Some(2024),
        year_to: Some(2024),
        ..Default::default()
    }), 1);
    let _ = all;
}

#[test]
fn source_info_is_written_per_draft_and_shown_in_detail() {
    let (_dir, db) = setup();
    let mut first = sample("bilibili", "1");
    first.media.source_url = Some("https://www.bilibili.com/bangumi/play/ss1".into());
    upsert(&db, &[first], "2026-11-25").unwrap();
    // 跨源合并（同标题同年）：两个来源的信息都留在详情
    let mut second = sample("iqiyi", "2");
    second.media.source_url = Some("https://www.iqiyi.com/v/2".into());
    second.media.episode_count = Some(12);
    upsert(&db, &[second], "2026-11-26").unwrap();
    let detail = detail(&db, "bilibili:1").unwrap().unwrap();
    assert_eq!(detail.source_infos.len(), 2, "两个来源各一条落地信息");
    let bilibili = detail.source_infos.iter().find(|s| s.source == "bilibili").unwrap();
    assert_eq!(bilibili.url.as_deref(), Some("https://www.bilibili.com/bangumi/play/ss1"));
    let iqiyi = detail.source_infos.iter().find(|s| s.source == "iqiyi").unwrap();
    assert_eq!(iqiyi.episode_text.as_deref(), Some("共 12 集"));
}

/// 直接插一条「升级前」形态的行：没有 norm_title、没有逐源信息与人员行。
fn insert_legacy_row(db: &Db, key: &str, source: &str, title: &str, year: Option<i32>, first: &str) {
    let media = CatalogMedia {
        key: key.into(),
        title: title.into(),
        kind: "anime".into(),
        source: source.into(),
        year,
        status: "airing".into(),
        platforms: vec![source.into()],
        first_seen: first.into(),
        updated_at: first.into(),
        ..Default::default()
    };
    db.write(|conn| {
        conn.execute(
            "INSERT INTO media(key,title,kind,status,year,source,first_seen,updated_at,search_text,payload,authority_json)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'',?9,'{}')",
            params![key, media.title, media.kind, media.status, media.year, media.source,
                media.first_seen, media.updated_at, serde_json::to_string(&media).unwrap()],
        )?;
        // 迁移路径本来就会写外部 ID 行；没有它，scope 过滤与合并搬移都无从谈起
        conn.execute(
            "INSERT INTO media_external_ids(source,external_id,media_key) VALUES (?1,?2,?3)
             ON CONFLICT DO NOTHING",
            params![source, key.rsplit_once(':').map(|(_, id)| id).unwrap_or(key), key],
        )?;
        Ok(())
    })
    .unwrap();
}

#[test]
fn dedupe_backfill_merges_same_year_and_marks_year_conflicts() {
    let (_dir, db) = setup();
    // 真正的「升级前」形态：同一部番在三个源各一行（直接 SQL 插入，无逐源信息）
    insert_legacy_row(&db, "bilibili:1", "bilibili", "同名番剧", Some(2026), "2026-11-25");
    insert_legacy_row(&db, "iqiyi:2", "iqiyi", "同名番剧", Some(2026), "2026-11-24");
    insert_legacy_row(&db, "tencent:3", "tencent", "同名番剧", Some(2025), "2026-11-25");
    let report = dedupe_backfill(&db, "2026-11-26").unwrap();
    assert_eq!(report.norm_backfilled, 3);
    assert_eq!(report.people_backfilled, 3);
    assert_eq!(report.source_info_backfilled, 3);
    // 同年（2026）聚类自动合并；2025 的年份冲突行自成一行并记疑似
    assert_eq!(count_all(&db).unwrap(), 2);
    assert_eq!(report.merged, 1);
    assert_eq!(report.suspects, 2, "冲突行与两个主行各记一条候选");
    // 主体 = first_seen 最早的行（iqiyi:2），bilibili 行并入它
    let detail = detail(&db, "iqiyi:2").unwrap().unwrap();
    assert!(detail.external_ids.iter().any(|id| id.source == "bilibili" && id.external_id == "1"));
    assert!(detail.source_infos.iter().any(|s| s.source == "bilibili"), "被合并行的来源信息要跟过来");
    assert_eq!(detail.source_infos.len(), 2, "iqiyi 自身 + 被合并的 bilibili");
    // 幂等：第二次跳过（标记已写）
    let again = dedupe_backfill(&db, "2026-11-27").unwrap();
    assert_eq!(again.merged, 0);
    assert_eq!(again.norm_backfilled, 0);
}

#[test]
fn overview_respects_scope_filter() {
    let (_dir, db) = setup();
    let mut domestic = sample("bilibili", "1");
    domestic.media.title = "国内番".into();
    let mut foreign = sample("anilist", "2");
    foreign.media.title = "海外番".into();
    upsert(&db, &[domestic, foreign], "2026-11-25").unwrap();
    let all = overview(&db, "2026-11-26", "2026-11-23", "2026-11-30", None).unwrap();
    assert_eq!(all.total, 2);
    let domestic = overview(&db, "2026-11-26", "2026-11-23", "2026-11-30", Some("domestic")).unwrap();
    assert_eq!(domestic.total, 1);
    let foreign = overview(&db, "2026-11-26", "2026-11-23", "2026-11-30", Some("foreign")).unwrap();
    assert_eq!(foreign.total, 1);
}

#[test]
fn follow_set_and_list_roundtrip() {
    let (_dir, db) = setup();
    upsert(&db, &[sample("bilibili", "1")], "2026-11-25").unwrap();
    let mut with_release = sample("iqiyi", "2");
    with_release.media.title = "另一部".into();
    with_release.releases.push(CatalogRelease {
        source: "iqiyi".into(),
        external_id: "2".into(),
        date: "2026-11-28".into(),
        episode: Some("3".into()),
        release_kind: "update".into(),
        updated_at: "2026-11-28T10:00:00Z".into(),
        ..Default::default()
    });
    upsert(&db, &[with_release], "2026-11-28").unwrap();

    // 收藏两部：一部在追、一部想看
    follow_set(&db, "bilibili:1", Some("watching"), "2026-11-28T09:00:00Z").unwrap();
    follow_set(&db, "iqiyi:2", Some("want"), "2026-11-28T09:00:00Z").unwrap();
    // 无效状态拒绝
    assert!(follow_set(&db, "bilibili:1", Some("bogus"), "2026-11-28").is_err());
    // 不存在的条目拒绝
    assert!(follow_set(&db, "bilibili:404", Some("watching"), "2026-11-28").is_err());

    let rows = follow_list(&db, &[]).unwrap();
    assert_eq!(rows.len(), 2);
    // 有发行的排在最前（latest DESC）
    assert_eq!(rows[0].media.key, "iqiyi:2");
    assert_eq!(rows[0].state, "want");
    assert_eq!(rows[0].latest_release.as_deref(), Some("2026-11-28"));
    assert_eq!(rows[0].latest_episode.as_deref(), Some("3"));
    // 状态筛选
    let watching = follow_list(&db, &["watching".into()]).unwrap();
    assert_eq!(watching.len(), 1);
    assert_eq!(watching[0].media.key, "bilibili:1");
    // 状态切换
    follow_set(&db, "bilibili:1", Some("done"), "2026-11-28T10:00:00Z").unwrap();
    let rows = follow_list(&db, &[]).unwrap();
    let changed = rows.iter().find(|r| r.media.key == "bilibili:1").unwrap();
    assert_eq!(changed.state, "done");
    // 取消收藏 = 删行
    follow_set(&db, "iqiyi:2", None, "2026-11-28T11:00:00Z").unwrap();
    assert_eq!(follow_list(&db, &[]).unwrap().len(), 1);
}

#[test]
fn merge_moves_follow_to_target_and_keeps_existing_state() {
    let (_dir, db) = setup();
    // 年份错开让两行共存（ingest 二级匹配只合并同年同题）
    let mut older = sample("iqiyi", "2");
    older.media.year = Some(2025);
    upsert(&db, &[sample("bilibili", "1"), older], "2026-11-25").unwrap();
    // 修正年份后跨源同题同年的行对，走 merge_candidates 人工合并路径
    db.write(|conn| {
        Ok(conn.execute("UPDATE media SET year=2026 WHERE key='iqiyi:2'", [])?)
    })
    .unwrap();

    // 分支 A：target 未收藏 → follow 整行搬移到 target
    follow_set(&db, "iqiyi:2", Some("want"), "2026-11-26").unwrap();
    merge_candidates(&db, "bilibili:1", "iqiyi:2", true, "2026-11-27").unwrap();
    let rows = follow_list(&db, &[]).unwrap();
    assert_eq!(rows.len(), 1, "follow 跟到 target，不产生第二行");
    assert_eq!(rows[0].media.key, "bilibili:1");
    assert_eq!(rows[0].state, "want");

    // 分支 B：target 已收藏 → 保留 target 的状态，败者收藏删除
    let mut older2 = sample("tencent", "3");
    older2.media.year = Some(2025);
    upsert(&db, &[older2], "2026-11-27").unwrap();
    follow_set(&db, "bilibili:1", Some("watching"), "2026-11-28").unwrap();
    follow_set(&db, "tencent:3", Some("done"), "2026-11-28").unwrap();
    merge_candidates(&db, "bilibili:1", "tencent:3", true, "2026-11-29").unwrap();
    let rows = follow_list(&db, &[]).unwrap();
    assert_eq!(rows.len(), 1, "合并后收藏行不能重复");
    assert_eq!(rows[0].media.key, "bilibili:1");
    assert_eq!(rows[0].state, "watching", "target 已收藏时保留 target 的状态");
}

