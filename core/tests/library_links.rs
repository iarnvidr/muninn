use muninn_core::library_links::{LinkInput, LinksImport};
use muninn_core::{Core, CoreConfig};

fn request(urls: &[&str], dry_run: bool) -> LinksImport {
    LinksImport {
        items: urls.iter().map(|s| LinkInput::Url(s.to_string())).collect(),
        collection: "Browser import".into(),
        dry_run,
    }
}

#[test]
fn imported_links_are_persistent_collection_items_without_tabs_media_or_download_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let open = || {
        Core::open(CoreConfig {
            data_dir: dir.path().into(),
            tool_dirs: Some(vec![]),
            ..Default::default()
        })
        .unwrap()
    };
    let core = open();
    let urls = [
        "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=30",
        "https://youtu.be/dQw4w9WgXcQ",
        "https://www.youtube.com/watch?v=aaaaaaaaaaa",
    ];
    let preview = core.import_links(request(&urls, true)).unwrap();
    assert_eq!(preview["added"], 2);
    assert!(core.library_view().unwrap().is_empty());
    assert!(core.organization().collections.is_empty());
    let imported = core.import_links(request(&urls, false)).unwrap();
    assert_eq!(imported["added"], 2);
    assert_eq!(imported["tabs_opened"], 0);
    assert_eq!(imported["downloads_queued"], 0);
    assert_eq!(
        core.import_links(request(&urls, false)).unwrap()["added"],
        0
    );
    assert!(core.video.list_entries().unwrap().is_empty());
    assert!(core.browsing.tabs().unwrap().is_empty());
    assert_eq!(core.organization().collections["Browser import"].len(), 2);
    assert!(core
        .library_view()
        .unwrap()
        .iter()
        .all(|v| v["state"] == "link" && v["size_bytes"] == 0));
    drop(core);
    let core = open();
    assert_eq!(core.library_view().unwrap().len(), 2);
    core.remove_library_link("dQw4w9WgXcQ").unwrap();
    assert_eq!(core.library_view().unwrap().len(), 1);
    assert_eq!(
        core.organization().collections["Browser import"],
        vec!["aaaaaaaaaaa"]
    );
}

#[test]
fn invalid_import_is_atomic_and_named_links_remain_visible_offline() {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::open(CoreConfig {
        data_dir: dir.path().into(),
        tool_dirs: Some(vec![]),
        ..Default::default()
    })
    .unwrap();
    assert!(core
        .import_links(request(
            &[
                "https://youtu.be/dQw4w9WgXcQ",
                "https://example.com/watch?v=aaaaaaaaaaa"
            ],
            false
        ))
        .is_err());
    assert!(core.library_view().unwrap().is_empty());
    assert!(core.organization().collections.is_empty());
    core.import_links(LinksImport {
        items: vec![LinkInput::Named {
            url: "https://youtu.be/dQw4w9WgXcQ".into(),
            title: Some("A saved video".into()),
        }],
        collection: "Browser import".into(),
        dry_run: false,
    })
    .unwrap();
    assert_eq!(core.library_view().unwrap()[0]["title"], "A saved video");
}

#[test]
fn large_link_collection_does_not_become_tabs_and_removing_reference_keeps_media() {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::open(CoreConfig {
        data_dir: dir.path().into(),
        tool_dirs: Some(vec![]),
        ..Default::default()
    })
    .unwrap();
    let items = (0..1000)
        .map(|n| LinkInput::Url(format!("https://youtu.be/{n:011}")))
        .collect();
    core.import_links(LinksImport {
        items,
        collection: "Archive".into(),
        dry_run: false,
    })
    .unwrap();
    assert_eq!(core.library_view().unwrap().len(), 1000);
    assert!(core.video.list_entries().unwrap().is_empty());
    assert!(core.browsing.tabs().unwrap().is_empty());
    let mut entry =
        muninn_core::videodb::EntryRow::new("00000000000", "720", "720", "import", core.now());
    entry.status = "complete".into();
    entry.protected = true;
    core.video.insert_entry(&entry).unwrap();
    assert_eq!(core.library_view().unwrap().len(), 1000);
    core.remove_library_link("00000000000").unwrap();
    assert!(core.video.get_entry(&entry.id).unwrap().unwrap().protected);
    assert_eq!(core.organization().collections["Archive"].len(), 999);
}
