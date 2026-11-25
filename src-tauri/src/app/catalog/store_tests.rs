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
fn shared_authoritative_ids_merge_but_title_year_and_season_do_not() {
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
    let mut other_year = sample("anilist", "11");
    other_year.media.year = Some(2025);
    let mut other_season = sample("anilist", "12");
    other_season.seasons.push(CatalogSeason {
        source: "anilist".into(),
        external_id: "s2".into(),
        number: Some(2),
        ..Default::default()
    });
    upsert(&db, &[other_year, other_season], "2026-11-22").unwrap();
    assert_eq!(count_all(&db).unwrap(), 3);
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
            genre: Some("Drama".into()),
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
    let facets = facets(&db).unwrap();
    assert_eq!(facets.kinds[0].label, "动漫");
    assert_eq!(facets.regions[0].label, "日本");
    assert_eq!(facets.genres[0].count, 3);
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
    upsert(
        &db,
        &[sample("anilist", "1"), sample("mal", "2")],
        "2026-11-22",
    )
    .unwrap();
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
    let overview = overview(&db, "2026-11-22", "2026-11-16", "2026-11-24").unwrap();
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
