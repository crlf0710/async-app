use proc_macro::{Span, TokenStream};
use quote::quote;
use syn::{ext::IdentExt};

#[proc_macro_attribute]
pub fn main(_args: TokenStream, input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as syn::ItemFn);
    if input.sig.ident.unraw() != "main" {
        return syn::Error::new_spanned(
            input.sig.ident,
            "Attribute `async_app::main` must be used on `main` function",
        ).to_compile_error().into();
    }
    if input.sig.asyncness.is_none() {
        return syn::Error::new_spanned(
            input.sig.ident,
            "Attribute `async_app::main` must be used on async function",
        ).to_compile_error().into();
    }
    let async_app_crate = syn::Ident::new("async_app", Span::call_site().into());
    let ret_type = match &input.sig.output {
        syn::ReturnType::Default => &syn::Type::Tuple(syn::TypeTuple{
            paren_token: syn::token::Paren::default(),
            elems: syn::punctuated::Punctuated::new(),
        }),
        syn::ReturnType::Type(_, ty) => &**ty,
    };
    let name = &input.sig.ident;
    let output = quote! {
        fn main() -> #ret_type {
            #input
            let scope = #async_app_crate::entryscope();
            let future = ::std::boxed::Box::pin((#name)(scope));
            #async_app_crate::entrypoint(future)
        }
    };
    output.into()
}
