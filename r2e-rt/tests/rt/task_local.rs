//! `task_local!` is reachable through the facade and scopes a value per task.

r2e_rt::task_local! {
    static CTX: u32;
}

#[test]
fn a_task_local_declared_through_the_facade_is_readable_inside_its_scope() {
    let runtime = r2e_rt::RuntimeBuilder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");

    let observed = runtime.block_on(CTX.scope(7, async { CTX.get() }));
    assert_eq!(observed, 7);

    // Outside any scope the key is unset.
    assert!(CTX.try_with(|v| *v).is_err());
}
