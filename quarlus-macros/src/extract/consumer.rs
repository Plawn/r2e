//! Consumer-related attribute extraction.

pub fn strip_consumer_attrs(attrs: Vec<syn::Attribute>) -> Vec<syn::Attribute> {
    attrs
        .into_iter()
        .filter(|a| !a.path().is_ident("consumer"))
        .collect()
}

pub fn extract_consumer(attrs: &[syn::Attribute]) -> syn::Result<Option<String>> {
    for attr in attrs {
        if attr.path().is_ident("consumer") {
            let mut bus_field = String::new();
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("bus") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    bus_field = lit.value();
                    Ok(())
                } else {
                    Err(meta.error("expected `bus`"))
                }
            })?;
            if bus_field.is_empty() {
                return Err(syn::Error::new_spanned(
                    attr,
                    "#[consumer] requires bus = \"field_name\"",
                ));
            }
            return Ok(Some(bus_field));
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
        "consumer parameter must be Arc<EventType>",
    ))
}
