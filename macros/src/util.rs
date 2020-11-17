use syn::{spanned::Spanned, Expr, ExprArray};

macro_rules! to_compile_error {
    ($span: expr, $msg: expr) => {
        Err(proc_macro::TokenStream::from(
            syn::Error::new($span, $msg).to_compile_error(),
        ))
    };
}

pub fn to_array(input: Expr) -> Result<ExprArray, proc_macro::TokenStream> {
    match input {
        Expr::Array(array) => Ok(array),
        _ => return to_compile_error!(input.span(), "config should be an array"),
    }
}
