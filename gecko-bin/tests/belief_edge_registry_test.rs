// The belief-edge registry accepts every extension `gecko-bin` compiles in, so a
// shipped declaration that the registry would refuse fails here rather than at the
// first `gecko run`.

#[cfg(feature = "cyber")]
#[test]
fn from_extensions_accepts_the_shipped_extensions() {
    use gecko_engine::extension::GeckoExtension;

    let exts: Vec<Box<dyn GeckoExtension>> = vec![mem_gecko::extension(), cyber_gecko::extension()];
    mem_gecko::BeliefEdgeRegistry::from_extensions(gecko_engine::db::CORE_SCHEMA, &exts)
        .expect("the shipped extensions register");
}
