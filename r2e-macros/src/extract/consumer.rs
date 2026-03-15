//! Consumer-related attribute extraction.

/// Parsed configuration from `#[consumer(...)]`.
pub struct ConsumerConfig {
    pub bus_field: String,
    pub topic: Option<String>,
    pub deserializer: Option<String>,
    pub filter: Option<String>,
    pub retry: Option<u32>,
    pub dlq: Option<String>,
}

pub fn strip_consumer_attrs(attrs: Vec<syn::Attribute>) -> Vec<syn::Attribute> {
    attrs
        .into_iter()
        .filter(|a| !a.path().is_ident("consumer"))
        .collect()
}

pub fn extract_consumer(attrs: &[syn::Attribute]) -> syn::Result<Option<ConsumerConfig>> {
    for attr in attrs {
        if attr.path().is_ident("consumer") {
            let mut bus_field = String::new();
            let mut topic = None;
            let mut deserializer = None;
            let mut filter = None;
            let mut retry = None;
            let mut dlq = None;
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("bus") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    bus_field = lit.value();
                    Ok(())
                } else if meta.path.is_ident("topic") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    topic = Some(lit.value());
                    Ok(())
                } else if meta.path.is_ident("deserializer") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    deserializer = Some(lit.value());
                    Ok(())
                } else if meta.path.is_ident("filter") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    filter = Some(lit.value());
                    Ok(())
                } else if meta.path.is_ident("retry") {
                    let value = meta.value()?;
                    let lit: syn::LitInt = value.parse()?;
                    retry = Some(lit.base10_parse::<u32>()?);
                    Ok(())
                } else if meta.path.is_ident("dlq") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    dlq = Some(lit.value());
                    Ok(())
                } else {
                    Err(meta.error(
                        "unknown key in #[consumer(...)]: expected `bus`, `topic`, `deserializer`, `filter`, `retry`, or `dlq`\n\
                         \n  usage: #[consumer(bus = \"event_bus\", topic = \"my-topic\")]"
                    ))
                }
            })?;
            if bus_field.is_empty() {
                return Err(syn::Error::new_spanned(
                    attr,
                    "#[consumer] requires `bus` — the name of the event bus field on the controller:\n\
                     \n  #[consumer(bus = \"event_bus\")]\n  async fn on_event(&self, event: Arc<MyEvent>) { }",
                ));
            }
            return Ok(Some(ConsumerConfig {
                bus_field,
                topic,
                deserializer,
                filter,
                retry,
                dlq,
            }));
        }
    }
    Ok(None)
}

pub fn extract_event_type_from_arc(ty: &syn::Type) -> syn::Result<syn::Type> {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            if segment.ident == "Arc" {
                if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                        return Ok(inner.clone());
                    }
                }
            }
        }
    }
    Err(syn::Error::new_spanned(
        ty,
        "consumer event parameter must be wrapped in Arc<T>:\n\
         \n  async fn on_event(&self, event: Arc<MyEvent>) { }\n\n\
         Events are shared across subscribers via Arc, so the parameter type must be Arc<YourEventType>.",
    ))
}
