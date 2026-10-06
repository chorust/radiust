use radiust_core::config::CoreConfig;
use radiust_core::engine::{Engine, EngineError};
use radiust_core::errors::CoreError;
use radiust_core::raster::AlphaPlane;
use radiust_core::source::SourceRegistry;

fn engine_with(config: CoreConfig) -> Engine {
    Engine::new(config, SourceRegistry::default()).unwrap()
}

#[tokio::test]
async fn gray_decode_checks_shape_overflow_pixel_limit_and_peak_buffer_budget() {
    let config = CoreConfig::default();
    let engine = engine_with(config.clone());
    let overflow = engine
        .decode_gray_values(usize::MAX, 2, Vec::new(), None, "gray-dbz-v1".into())
        .await
        .unwrap_err();
    assert!(matches!(overflow, EngineError::Core(CoreError::ResourceLimit(_))));

    let mut small = config.clone();
    small.runtime.max_pixels = 3;
    let engine = engine_with(small);
    let too_many = engine
        .decode_gray_values(2, 2, vec![0.0; 4], None, "gray-dbz-v1".into())
        .await
        .unwrap_err();
    assert!(matches!(too_many, EngineError::Core(CoreError::ResourceLimit(_))));

    let mut low_memory = config;
    low_memory.runtime.max_temp_bytes = 32;
    let engine = engine_with(low_memory);
    let over_budget = engine
        .decode_gray_values(
            2,
            2,
            vec![0.0; 4],
            Some(AlphaPlane::U16(vec![1; 4])),
            "gray-dbz-v1".into(),
        )
        .await
        .unwrap_err();
    assert!(matches!(over_budget, EngineError::Core(CoreError::ResourceLimit(_))));
}

#[tokio::test]
async fn cancelled_engine_does_not_start_gray_decode_worker() {
    let engine = engine_with(CoreConfig::default());
    engine.cancel();
    let error =
        engine.decode_gray_values(1, 1, vec![0.0], None, "gray-dbz-v1".into()).await.unwrap_err();
    assert!(matches!(error, EngineError::Core(CoreError::Cancelled)));
}

#[test]
fn gray_decode_deadline_expires_while_cpu_worker_is_queued() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .start_paused(true)
        .build()
        .unwrap();
    let result = runtime.block_on(async {
        let (release, wait_for_release) = std::sync::mpsc::channel::<()>();
        let (started, wait_for_start) = tokio::sync::oneshot::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            let _ = started.send(());
            // Dropping the sender also releases this worker if the test panics.
            let _ = wait_for_release.recv();
        });
        wait_for_start.await.unwrap();

        let mut config = CoreConfig::default();
        config.runtime.frame_deadline = 0.1;
        let engine = engine_with(config);
        let decode =
            engine.decode_gray_values(64, 64, vec![0.0; 64 * 64], None, "gray-dbz-v1".into());
        tokio::pin!(decode);
        // Start the operation and its timer while the blocking pool is occupied.
        assert!(futures_util::poll!(&mut decode).is_pending());
        tokio::time::advance(std::time::Duration::from_secs(1)).await;
        let result = decode.await;
        drop(release);
        blocker.await.unwrap();
        result
    });
    let error = result.unwrap_err();
    assert!(matches!(
        error,
        EngineError::Core(CoreError::Transport(message)) if message.contains("deadline")
    ));
}
