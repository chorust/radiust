use radiust_core::storage::object::ObjectStore;

#[tokio::test]
async fn memory_object_store_streams_and_hashes_content() {
    let operator = opendal::Operator::new(opendal::services::Memory::default()).unwrap();
    let store = ObjectStore::from_operator(operator, "generation-a").unwrap();

    let written = store
        .write_chunks("artifact.bin", vec![b"hello ".to_vec(), b"world".to_vec()], 64)
        .await
        .unwrap();
    let (payload, read) = store.read("artifact.bin", 64).await.unwrap();

    assert_eq!(payload, b"hello world");
    assert_eq!(written, read);
    assert_eq!(read.size_bytes, 11);
    assert_eq!(read.sha256, "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9");
}

#[tokio::test]
async fn object_store_rejects_unsafe_keys_and_size_overflow() {
    let operator = opendal::Operator::new(opendal::services::Memory::default()).unwrap();
    let store = ObjectStore::from_operator(operator, "generation-a").unwrap();

    assert!(store.read("../escape", 64).await.is_err());
    assert!(store.write_chunks("too-large", vec![vec![0; 65]], 64).await.is_err());
}

#[tokio::test]
async fn object_store_optional_reads_and_single_publish_preserve_existing_objects() {
    let operator = opendal::Operator::new(opendal::services::Memory::default()).unwrap();
    let store = ObjectStore::from_operator(operator, "generation-a").unwrap();

    assert!(store.read_optional("missing.bin", 64).await.unwrap().is_none());
    store.write_chunks_once("artifact.bin", vec![b"first".to_vec()], 64).await.unwrap();
    assert!(store.write_chunks_once("artifact.bin", vec![b"second".to_vec()], 64).await.is_err());
    assert_eq!(store.read("artifact.bin", 64).await.unwrap().0, b"first");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_create_only_writes_have_exactly_one_winner() {
    let operator = opendal::Operator::new(opendal::services::Memory::default()).unwrap();
    let store = ObjectStore::from_operator(operator, "generation-a").unwrap();
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(12));
    let tasks = (0..12).map(|index| {
        let store = store.clone();
        let barrier = barrier.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            let payload = format!("writer-{index}").into_bytes();
            let result = store.write_chunks_once("shared.bin", vec![payload.clone()], 64).await;
            (payload, result)
        })
    });
    let results = futures_util::future::join_all(tasks).await;
    let winners: Vec<_> = results
        .into_iter()
        .map(Result::unwrap)
        .filter_map(|(payload, result)| result.ok().map(|_| payload))
        .collect();
    assert_eq!(winners.len(), 1);
    assert_eq!(store.read("shared.bin", 64).await.unwrap().0, winners[0]);
}
