use proc_macro::TokenStream;
use quote::{quote, quote_spanned};
use syn::{Ident, Token, parse_macro_input, punctuated::Punctuated};

#[proc_macro_attribute]
pub fn portable(attribute: TokenStream, item: TokenStream) -> TokenStream {
    let options = parse_macro_input!(
        attribute with Punctuated::<Ident, Token![,]>::parse_terminated
    );
    let mut copy = false;
    let mut custom_debug = false;
    let mut custom_deserialize = false;
    let mut default = false;
    let mut eq = false;
    let mut hash = false;
    let mut ord = false;

    for option in options {
        match option.to_string().as_str() {
            "copy" => copy = true,
            "custom_debug" => custom_debug = true,
            "custom_deserialize" => custom_deserialize = true,
            "default" => default = true,
            "eq" => eq = true,
            "hash" => hash = true,
            "ord" => ord = true,
            _ => {
                return quote_spanned! {
                    option.span()=> compile_error!("unsupported portable model capability");
                }
                .into();
            }
        }
    }

    let copy = copy.then(|| quote!(Copy,));
    let debug = (!custom_debug).then(|| quote!(Debug,));
    let deserialize = (!custom_deserialize).then(|| quote!(::serde::Deserialize,));
    let default = default.then(|| quote!(Default,));
    let eq = (eq || hash || ord).then(|| quote!(Eq,));
    let hash = hash.then(|| quote!(Hash,));
    let ordering = ord.then(|| quote!(Ord, PartialOrd,));
    let item: proc_macro2::TokenStream = item.into();

    quote! {
        #[derive(
            Clone,
            #copy
            #debug
            #default
            #eq
            #hash
            #ordering
            PartialEq,
            #deserialize
            ::serde::Serialize,
        )]
        #item
    }
    .into()
}
