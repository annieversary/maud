use proc_macro2::TokenStream;
use quote::quote;

use crate::ast::*;

pub fn zephyr(markups: &[Markup]) -> Option<TokenStream> {
    let all_lit_classes = markups
        .iter()
        .flat_map(|m| get_single_or_block_element_attrs(m))
        .flat_map(|a| {
            if let Attr::Class {
                name,
                toggler: None,
                ..
            } = a
            {
                return get_single_or_block_literal(name);
            } else if let Attr::Named { named_attr } = a {
                if let AttrType::Normal { value } = &named_attr.attr_type {
                    if name_to_string(named_attr.name.clone()).trim() == "class" {
                        return get_single_or_block_literal(value);
                    }
                }
            }
            None
        })
        .collect::<Vec<_>>()
        .join(" ");

    if !all_lit_classes.is_empty() {
        Some(quote! {
            maud::zephyr::register_class!(#all_lit_classes);
        })
    } else {
        None
    }
}

fn get_single_or_block_element_attrs(markup: &Markup) -> Vec<&Attr> {
    match markup {
        Markup::Block(block) => block
            .markups
            .iter()
            .flat_map(get_single_or_block_element_attrs)
            .collect::<Vec<_>>(),
        Markup::Element { attrs, body, .. } => {
            let mut out: Vec<_> = attrs.iter().collect();
            if let ElementBody::Block { block } = body {
                out.extend(
                    block
                        .markups
                        .iter()
                        .flat_map(get_single_or_block_element_attrs),
                )
            }

            out
        }
        Markup::Special { segments } => segments
            .iter()
            .flat_map(|s| &s.body.markups)
            .flat_map(get_single_or_block_element_attrs)
            .collect(),
        Markup::Match { arms, .. } => arms
            .iter()
            .flat_map(|s| &s.body.markups)
            .flat_map(get_single_or_block_element_attrs)
            .collect(),
        _ => vec![],
    }
}

fn get_single_or_block_literal(markup: &Markup) -> Option<String> {
    match markup {
        Markup::Block(block) => Some(
            block
                .markups
                .iter()
                .flat_map(get_single_or_block_literal)
                .collect::<Vec<_>>()
                .join(" "),
        ),
        Markup::Literal { content, .. } => Some(content.to_string()),
        Markup::Symbol { symbol } => Some(
            symbol
                .clone()
                .into_iter()
                .map(|a| a.to_string())
                .collect::<String>(),
        ),
        _ => None,
    }
}
