//! Report what AutoLog can and cannot do with this machine's credential store.
//!
//! Run with `cargo run -p autolog-core --example keyring_check`. It writes,
//! reads back and deletes one throwaway entry, so a failure here is the same
//! failure the app would hit when saving a token.

use autolog_core::secrets::{Secret, SecretStore};

fn main() {
    println!("store status: {:?}", keyring::Entry::store_status());

    let store = SecretStore::new();
    match store.unavailable() {
        Some(error) => println!("AutoLog sees: UNAVAILABLE - {error}"),
        None => println!("AutoLog sees: available"),
    }

    let consumer = "keyring-check";
    let field = "probe";

    println!("\nbefore writing: {:?}", store.get(consumer, field));

    match store.set(consumer, field, &Secret::new("hunter2")) {
        Ok(()) => println!("write: ok"),
        Err(error) => println!("write: FAILED - {error}"),
    }

    let read = store.get(consumer, field);
    println!("read back:      {read:?} (empty = {})", read.is_empty());
    if read.expose() == "hunter2" {
        println!("round trip:     ok");
    } else {
        // Deliberately not printed: whatever came back is somebody's secret,
        // even when it is not the one this probe wrote.
        println!(
            "round trip:     FAILED - read back {} characters, not the probe value",
            read.expose().len()
        );
    }

    report_app_entries(&store);

    match store.set(consumer, field, &Secret::default()) {
        Ok(()) => println!("cleanup:        ok"),
        Err(error) => println!("cleanup:        FAILED - {error}"),
    }
}

/// What AutoLog would find for each consumer's own fields.
///
/// Reports only whether something is stored and how long it is. The values
/// themselves are never printed.
fn report_app_entries(store: &SecretStore) {
    println!("\nstored for AutoLog's own consumers:");
    for spec in autolog_core::consumer::CONSUMERS.iter() {
        for field in spec.credentials.iter().filter(|field| field.secret) {
            let found = store.get(spec.id, field.key);
            println!(
                "  {:6} {:10} present={} length={}",
                spec.id,
                field.key,
                !found.is_empty(),
                found.expose().len()
            );
        }
    }
}
