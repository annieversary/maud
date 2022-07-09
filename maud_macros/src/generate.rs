use proc_macro2::{Delimiter, Group, Ident, Literal, Span, TokenStream, TokenTree};
use proc_macro_error::SpanRange;
use quote::quote;

use crate::{ast::*, escape};

pub fn generate(markups: Vec<Markup>, output_ident: TokenTree) -> TokenStream {
    let mut build = Builder::new(output_ident.clone());
    Generator::new(output_ident).markups(markups, &mut build);
    build.finish()
}

struct Generator {
    output_ident: TokenTree,
}

impl Generator {
    fn new(output_ident: TokenTree) -> Generator {
        Generator { output_ident }
    }

    fn builder(&self) -> Builder {
        Builder::new(self.output_ident.clone())
    }

    fn markups(&self, markups: Vec<Markup>, build: &mut Builder) {
        for markup in markups {
            self.markup(markup, build);
        }
    }

    fn markup(&self, markup: Markup, build: &mut Builder) {
        match markup {
            Markup::ParseError { .. } => {}
            Markup::Block(Block {
                markups,
                outer_span,
            }) => {
                if markups
                    .iter()
                    .any(|markup| matches!(*markup, Markup::Let { .. }))
                {
                    self.block(
                        Block {
                            markups,
                            outer_span,
                        },
                        build,
                    );
                } else {
                    self.markups(markups, build);
                }
            }
            Markup::Literal { content, .. } => build.push_escaped(&content),
            Markup::Symbol { symbol } => self.name(symbol, build),
            Markup::Splice { expr, .. } => self.splice(expr, build),
            Markup::Element { name, attrs, body } => self.element(name, attrs, body, build),
            Markup::Let { tokens, .. } => build.push_tokens(tokens),
            Markup::Special { segments } => {
                for Special { head, body, .. } in segments {
                    build.push_tokens(head);
                    self.block(body, build);
                }
            }
            Markup::Custom {
                name, expr, body, ..
            } => self.custom_element(name, expr, body, build),
            Markup::Match {
                head,
                arms,
                arms_span,
                ..
            } => {
                let body = {
                    let mut build = self.builder();
                    for MatchArm { head, body } in arms {
                        build.push_tokens(head);
                        self.block(body, &mut build);
                    }
                    build.finish()
                };
                let mut body = TokenTree::Group(Group::new(Delimiter::Brace, body));
                body.set_span(arms_span.collapse());
                build.push_tokens(quote!(#head #body));
            }
        }
    }

    fn block(
        &self,
        Block {
            markups,
            outer_span,
        }: Block,
        build: &mut Builder,
    ) {
        let block = {
            let mut build = self.builder();
            self.markups(markups, &mut build);
            build.finish()
        };
        let mut block = TokenTree::Group(Group::new(Delimiter::Brace, block));
        block.set_span(outer_span.collapse());
        build.push_tokens(TokenStream::from(block));
    }

    fn splice(&self, expr: TokenStream, build: &mut Builder) {
        let output_ident = self.output_ident.clone();
        build.push_tokens(quote!(maud::Render::render_to(&#expr, &mut #output_ident);));
    }

    fn custom_element(
        &self,
        name: TokenStream,
        expr: TokenStream,
        body: ElementBody,
        build: &mut Builder,
    ) {
        let output_ident = self.output_ident.clone();

        if let ElementBody::Block { block } = body {
            let inner = {
                let mut build = Builder::new(output_ident.clone());
                Generator::new(output_ident.clone()).markups(block.markups, &mut build);
                let stmts = build.finish();
                quote!({
                    // TODO add something to predict size of this
                    let mut #output_ident = String::with_capacity(20);
                    #stmts
                    maud::PreEscaped(#output_ident)
                })
            };
            build.push_tokens(match expr.clone().into_iter().last() {
                Some(TokenTree::Punct(p)) if p.as_char() == ',' => {
                    quote!(maud::Render::render_to(&#name(#expr #inner), &mut #output_ident);)
                }
                _ => {
                    quote!(maud::Render::render_to(&#name(#expr, #inner), &mut #output_ident);)
                }
            });
        } else {
            build.push_tokens(quote!(maud::Render::render_to(&#name(#expr), &mut #output_ident);));
        }
    }

    fn element(
        &self,
        name: TokenStream,
        #[allow(unused_mut)] mut attrs: Vec<Attr>,
        body: ElementBody,
        build: &mut Builder,
    ) {
        // add `rel="noopener noreferrer"` to all `a` tags that don't have a `rel` attribute
        #[cfg(feature = "a_rel_attribute")]
        if name_to_string(name.clone()).as_str() == "a" {
            let mut has_rel = false;
            for a in &attrs {
                if let Attr::Named {
                    named_attr: NamedAttr { name, .. },
                } = a
                {
                    if name_to_string(name.clone()).as_str() == "rel" {
                        has_rel = true;
                    }
                }
            }

            if !has_rel {
                // this is not accurate at all, but i do not give a shit
                let span = match &body {
                    ElementBody::Void { semi_span } => *semi_span,
                    ElementBody::Block { block } => block.outer_span,
                };

                attrs.push(Attr::Named {
                    named_attr: NamedAttr {
                        name: quote!(rel),
                        attr_type: AttrType::Normal {
                            value: Markup::Literal {
                                content: "noopener noreferrer".into(),
                                span,
                            },
                        },
                    },
                });
            }
        }

        build.push_str("<");
        self.name(name.clone(), build);
        self.attrs(attrs, build);
        build.push_str(">");
        if let ElementBody::Block { block } = body {
            self.markups(block.markups, build);
            build.push_str("</");
            self.name(name, build);
            build.push_str(">");
        }
    }

    fn name(&self, name: TokenStream, build: &mut Builder) {
        build.push_escaped(&name_to_string(name));
    }

    fn attrs(&self, attrs: Vec<Attr>, build: &mut Builder) {
        for NamedAttr { name, attr_type } in desugar_attrs(attrs) {
            match attr_type {
                AttrType::Normal { value } => {
                    build.push_str(" ");
                    self.name(name, build);
                    build.push_str("=\"");
                    self.markup(value, build);
                    build.push_str("\"");
                }
                AttrType::Optional {
                    toggler: Toggler { cond, .. },
                } => {
                    let inner_value = quote!(inner_value);
                    let body = {
                        let mut build = self.builder();
                        build.push_str(" ");
                        self.name(name, &mut build);
                        build.push_str("=\"");
                        self.splice(inner_value.clone(), &mut build);
                        build.push_str("\"");
                        build.finish()
                    };
                    build.push_tokens(quote!(if let Some(#inner_value) = (#cond) { #body }));
                }
                AttrType::Empty { toggler: None } => {
                    build.push_str(" ");
                    self.name(name, build);
                }
                AttrType::Empty {
                    toggler: Some(Toggler { cond, .. }),
                } => {
                    let body = {
                        let mut build = self.builder();
                        build.push_str(" ");
                        self.name(name, &mut build);
                        build.finish()
                    };
                    build.push_tokens(quote!(if (#cond) { #body }));
                }
            }
        }
    }
}

////////////////////////////////////////////////////////

fn desugar_attrs(attrs: Vec<Attr>) -> Vec<NamedAttr> {
    let mut classes_static = vec![];
    let mut classes_toggled = vec![];
    let mut ids = vec![];
    let mut named_attrs = vec![];
    for attr in attrs {
        match attr {
            Attr::Class {
                name,
                toggler: Some(toggler),
                ..
            } => classes_toggled.push((name, toggler)),
            Attr::Class {
                name,
                toggler: None,
                ..
            } => classes_static.push(name),
            Attr::Id { name, .. } => ids.push(name),
            Attr::Named { named_attr } => named_attrs.push(named_attr),
        }
    }
    let classes = desugar_classes_or_ids("class", classes_static, classes_toggled);
    let ids = desugar_classes_or_ids("id", ids, vec![]);
    classes.into_iter().chain(ids).chain(named_attrs).collect()
}

fn desugar_classes_or_ids(
    attr_name: &'static str,
    values_static: Vec<Markup>,
    values_toggled: Vec<(Markup, Toggler)>,
) -> Option<NamedAttr> {
    if values_static.is_empty() && values_toggled.is_empty() {
        return None;
    }
    let mut markups = Vec::new();
    let mut leading_space = false;
    for name in values_static {
        markups.extend(prepend_leading_space(name, &mut leading_space));
    }
    for (name, Toggler { cond, cond_span }) in values_toggled {
        let body = Block {
            markups: prepend_leading_space(name, &mut leading_space),
            // TODO: is this correct?
            outer_span: cond_span,
        };
        markups.push(Markup::Special {
            segments: vec![Special {
                at_span: SpanRange::call_site(),
                head: quote!(if (#cond)),
                body,
            }],
        });
    }
    Some(NamedAttr {
        name: TokenStream::from(TokenTree::Ident(Ident::new(attr_name, Span::call_site()))),
        attr_type: AttrType::Normal {
            value: Markup::Block(Block {
                markups,
                outer_span: SpanRange::call_site(),
            }),
        },
    })
}

fn prepend_leading_space(name: Markup, leading_space: &mut bool) -> Vec<Markup> {
    let mut markups = Vec::new();
    if *leading_space {
        markups.push(Markup::Literal {
            content: " ".to_owned(),
            span: name.span(),
        });
    }
    *leading_space = true;
    markups.push(name);
    markups
}

////////////////////////////////////////////////////////

struct Builder {
    output_ident: TokenTree,
    tokens: Vec<TokenTree>,
    tail: String,
}

impl Builder {
    fn new(output_ident: TokenTree) -> Builder {
        Builder {
            output_ident,
            tokens: Vec::new(),
            tail: String::new(),
        }
    }

    fn push_str(&mut self, string: &str) {
        self.tail.push_str(string);
    }

    fn push_escaped(&mut self, string: &str) {
        escape::escape_to_string(string, &mut self.tail);
    }

    fn push_tokens(&mut self, tokens: TokenStream) {
        self.cut();
        self.tokens.extend(tokens);
    }

    fn cut(&mut self) {
        if self.tail.is_empty() {
            return;
        }
        let push_str_expr = {
            let output_ident = self.output_ident.clone();
            let string = TokenTree::Literal(Literal::string(&self.tail));
            quote!(#output_ident.push_str(#string);)
        };
        self.tail.clear();
        self.tokens.extend(push_str_expr);
    }

    fn finish(mut self) -> TokenStream {
        self.cut();
        self.tokens.into_iter().collect()
    }
}
