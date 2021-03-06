use peekaboo::*;
use proc_macro2::{Delimiter, Ident, Literal, Spacing, Span, TokenStream, TokenTree};
use proc_macro_error::{abort, abort_call_site, emit_error, SpanRange};
use quote::ToTokens;
use std::collections::HashMap;

use syn::Lit;

use crate::ast;

pub fn parse(input: TokenStream) -> Vec<ast::Markup> {
    Parser::new(input).markups()
}

#[derive(Clone)]
struct Parser {
    /// If we're inside an attribute, then this contains the attribute name.
    current_attr: Option<String>,
    input: Peekable<<TokenStream as IntoIterator>::IntoIter, 2>,
}

impl Iterator for Parser {
    type Item = TokenTree;

    fn next(&mut self) -> Option<TokenTree> {
        self.input.next()
    }
}

impl Parser {
    fn new(input: TokenStream) -> Parser {
        Parser {
            current_attr: None,
            input: input.into_iter().peekable_n(),
        }
    }

    fn with_input(&self, input: TokenStream) -> Parser {
        Parser {
            current_attr: self.current_attr.clone(),
            input: input.into_iter().peekable_n(),
        }
    }

    /// Returns the next token in the stream without consuming it.
    fn peek(&mut self) -> Option<&TokenTree> {
        self.input.peek::<1>()
    }

    /// Returns the next two tokens in the stream without consuming them.
    fn peek2(&mut self) -> Option<(&TokenTree, Option<&TokenTree>)> {
        let [peek1, peek2] = self.input.peek_multiple::<2>();

        peek1.map(|first| (first, peek2))
    }

    /// Advances the cursor by one step.
    fn advance(&mut self) {
        self.next();
    }

    /// Advances the cursor by two steps.
    fn advance2(&mut self) {
        self.next();
        self.next();
    }

    /// Parses multiple blocks of markup.
    fn markups(&mut self) -> Vec<ast::Markup> {
        let mut result = Vec::new();
        loop {
            match self.peek2() {
                None => break,
                Some((TokenTree::Punct(ref punct), _)) if punct.as_char() == ';' => self.advance(),
                Some((TokenTree::Punct(punct), Some(TokenTree::Ident(ident))))
                    if punct.as_char() == '@' && *ident == "let" =>
                {
                    let keyword = TokenTree::Ident(ident.clone());
                    let span = punct.span();
                    self.advance2();
                    result.push(self.let_expr(span, keyword));
                }
                _ => result.push(self.markup()),
            }
        }
        result
    }

    /// Parses a single block of markup.
    fn markup(&mut self) -> ast::Markup {
        let token = match self.peek() {
            Some(token) => token,
            None => {
                abort_call_site!("unexpected end of input");
            }
        };
        let markup = match token {
            // Literal
            TokenTree::Literal(literal) => {
                let literal = literal.clone();
                self.advance();
                self.literal(literal)
            }
            // Special form
            TokenTree::Punct(ref punct) if punct.as_char() == '@' => {
                let at_span = punct.span();
                self.advance();
                match self.next() {
                    Some(TokenTree::Ident(ident)) => {
                        let keyword = TokenTree::Ident(ident.clone());
                        match ident.to_string().as_str() {
                            "if" => {
                                let mut segments = Vec::new();
                                self.if_expr(at_span, vec![keyword], &mut segments);
                                ast::Markup::Special { segments }
                            }
                            "while" => self.while_expr(at_span, keyword),
                            "for" => self.for_expr(at_span, keyword),
                            "match" => self.match_expr(at_span, keyword),
                            "let" => {
                                let span = SpanRange {
                                    first: at_span,
                                    last: ident.span(),
                                };
                                abort!(span, "`@let` only works inside a block");
                            }
                            // TODO add components here
                            _ => self.custom_component(at_span, keyword),
                        }
                    }
                    _ => {
                        abort!(at_span, "expected keyword after `@`");
                    }
                }
            }
            // Element
            TokenTree::Ident(ident) => {
                let ident = ident.clone(); // we could probably skip this clone but i cant be bothered
                let ident_string = ident.to_string();
                let span = SpanRange::single_span(ident.span());
                match ident_string.as_str() {
                    "if" | "while" | "for" | "match" | "let" => {
                        abort!(
                            ident,
                            "found keyword `{}`", ident_string;
                            help = "should this be a `@{}`?", ident_string
                        );
                    }
                    "true" | "false" => {
                        if let Some(attr_name) = &self.current_attr {
                            emit_error!(
                                ident,
                                r#"attribute value must be a string"#;
                                help = "to declare an empty attribute, omit the equals sign: `{}`",
                                attr_name;
                                help = "to toggle the attribute, use square brackets: `{}[some_boolean_flag]`",
                                attr_name;
                            );
                            return ast::Markup::ParseError { span };
                        }
                    }
                    _ => {}
                }

                // `.try_namespaced_name()` should never fail as we've
                // already seen an `Ident`
                let name = self.try_namespaced_name().expect("identifier");
                self.element(name)
            }
            // Div element shorthand
            TokenTree::Punct(ref punct) if punct.as_char() == '.' || punct.as_char() == '#' => {
                let name = TokenTree::Ident(Ident::new("div", punct.span()));
                self.element(name.into())
            }
            // Splice
            TokenTree::Group(ref group) if group.delimiter() == Delimiter::Parenthesis => {
                let expr = group.stream();
                let outer_span = SpanRange::single_span(group.span());
                self.advance();
                ast::Markup::Splice { expr, outer_span }
            }
            // Block
            TokenTree::Group(ref group) if group.delimiter() == Delimiter::Brace => {
                let stream = group.stream();
                let span = SpanRange::single_span(group.span());
                self.advance();
                ast::Markup::Block(self.block(stream, span))
            }
            // ???
            token => {
                abort!(token, "invalid syntax");
            }
        };
        markup
    }

    /// Parses a literal string.
    fn literal(&mut self, literal: Literal) -> ast::Markup {
        match Lit::new(literal.clone()) {
            Lit::Str(lit_str) => {
                return ast::Markup::Literal {
                    content: lit_str.value(),
                    span: SpanRange::single_span(literal.span()),
                }
            }
            // Boolean literals are idents, so `Lit::Bool` is handled in
            // `markup`, not here.
            Lit::Int(..) | Lit::Float(..) => {
                emit_error!(literal, r#"literal must be double-quoted: `"{}"`"#, literal);
            }
            Lit::Char(lit_char) => {
                emit_error!(
                    literal,
                    r#"literal must be double-quoted: `"{}"`"#,
                    lit_char.value(),
                );
            }
            _ => {
                emit_error!(literal, "expected string");
            }
        }
        ast::Markup::ParseError {
            span: SpanRange::single_span(literal.span()),
        }
    }

    /// Parses an `@if` expression.
    ///
    /// The leading `@if` should already be consumed.
    fn if_expr(&mut self, at_span: Span, prefix: Vec<TokenTree>, segments: &mut Vec<ast::Special>) {
        let mut head = prefix;
        let body = loop {
            match self.next() {
                Some(TokenTree::Group(ref block)) if block.delimiter() == Delimiter::Brace => {
                    break self.block(block.stream(), SpanRange::single_span(block.span()));
                }
                Some(token) => head.push(token),
                None => {
                    let mut span = ast::span_tokens(head);
                    span.first = at_span;
                    abort!(span, "expected body for this `@if`");
                }
            }
        };
        segments.push(ast::Special {
            at_span: SpanRange::single_span(at_span),
            head: head.into_iter().collect(),
            body,
        });
        self.else_if_expr(segments)
    }

    /// Parses an optional `@else if` or `@else`.
    ///
    /// The leading `@else if` or `@else` should *not* already be consumed.
    fn else_if_expr(&mut self, segments: &mut Vec<ast::Special>) {
        match self.peek2() {
            Some((TokenTree::Punct(ref punct), Some(TokenTree::Ident(ref else_keyword))))
                if punct.as_char() == '@' && *else_keyword == "else" =>
            {
                let at_span = punct.span();
                let else_keyword = TokenTree::Ident(else_keyword.clone());
                self.advance2();
                match self.peek() {
                    // `@else if`
                    Some(TokenTree::Ident(ref if_keyword)) if *if_keyword == "if" => {
                        let if_keyword = TokenTree::Ident(if_keyword.clone());
                        self.advance();
                        self.if_expr(at_span, vec![else_keyword, if_keyword], segments)
                    }
                    // Just an `@else`
                    _ => match self.next() {
                        Some(TokenTree::Group(ref group))
                            if group.delimiter() == Delimiter::Brace =>
                        {
                            let body =
                                self.block(group.stream(), SpanRange::single_span(group.span()));
                            segments.push(ast::Special {
                                at_span: SpanRange::single_span(at_span),
                                head: vec![else_keyword].into_iter().collect(),
                                body,
                            });
                        }
                        _ => {
                            let span = SpanRange {
                                first: at_span,
                                last: else_keyword.span(),
                            };
                            abort!(span, "expected body for this `@else`");
                        }
                    },
                }
            }
            // We didn't find an `@else`; stop
            _ => {}
        }
    }

    /// Parses an `@while` expression.
    ///
    /// The leading `@while` should already be consumed.
    fn while_expr(&mut self, at_span: Span, keyword: TokenTree) -> ast::Markup {
        let keyword_span = keyword.span();
        let mut head = vec![keyword];
        let body = loop {
            match self.next() {
                Some(TokenTree::Group(ref block)) if block.delimiter() == Delimiter::Brace => {
                    break self.block(block.stream(), SpanRange::single_span(block.span()));
                }
                Some(token) => head.push(token),
                None => {
                    let span = SpanRange {
                        first: at_span,
                        last: keyword_span,
                    };
                    abort!(span, "expected body for this `@while`");
                }
            }
        };
        ast::Markup::Special {
            segments: vec![ast::Special {
                at_span: SpanRange::single_span(at_span),
                head: head.into_iter().collect(),
                body,
            }],
        }
    }

    /// Parses a `@for` expression.
    ///
    /// The leading `@for` should already be consumed.
    fn for_expr(&mut self, at_span: Span, keyword: TokenTree) -> ast::Markup {
        let keyword_span = keyword.span();
        let mut head = vec![keyword];
        loop {
            match self.next() {
                Some(TokenTree::Ident(ref in_keyword)) if *in_keyword == "in" => {
                    head.push(TokenTree::Ident(in_keyword.clone()));
                    break;
                }
                Some(token) => head.push(token),
                None => {
                    let span = SpanRange {
                        first: at_span,
                        last: keyword_span,
                    };
                    abort!(span, "missing `in` in `@for` loop");
                }
            }
        }
        let body = loop {
            match self.next() {
                Some(TokenTree::Group(ref block)) if block.delimiter() == Delimiter::Brace => {
                    break self.block(block.stream(), SpanRange::single_span(block.span()));
                }
                Some(token) => head.push(token),
                None => {
                    let span = SpanRange {
                        first: at_span,
                        last: keyword_span,
                    };
                    abort!(span, "expected body for this `@for`");
                }
            }
        };
        ast::Markup::Special {
            segments: vec![ast::Special {
                at_span: SpanRange::single_span(at_span),
                head: head.into_iter().collect(),
                body,
            }],
        }
    }

    fn custom_component(&mut self, at_span: Span, name: TokenTree) -> ast::Markup {
        let at_span = SpanRange::single_span(at_span);

        let group = if let Some(TokenTree::Group(group)) = self.next() {
            if group.delimiter() == Delimiter::Parenthesis {
                group
            } else {
                abort!(at_span, "expected parentesised group");
            }
        } else {
            abort!(at_span, "expected parentesised group");
        };
        let expr = group.stream();

        let body = match self.peek() {
            Some(TokenTree::Punct(ref punct))
                if punct.as_char() == ';' || punct.as_char() == '/' =>
            {
                // Void element
                let c = punct.as_char();
                let semi_span = SpanRange::single_span(punct.span());
                if c == '/' {
                    emit_error!(
                        punct,
                        "void elements must use `;`, not `/`";
                        help = "change this to `;`";
                        help = "see https://github.com/lambda-fairy/maud/pull/315 for details";
                    );
                }
                self.advance();
                ast::ElementBody::Void { semi_span }
            }
            Some(_) => match self.markup() {
                ast::Markup::Block(block) => ast::ElementBody::Block { block },
                markup => {
                    let markup_span = markup.span();
                    abort!(
                        markup_span,
                        "element body must be wrapped in braces";
                        help = "see https://github.com/lambda-fairy/maud/pull/137 for details"
                    );
                }
            },
            None => abort_call_site!("expected `;`, found end of macro"),
        };

        ast::Markup::Custom {
            at_span,
            name: name.into_token_stream(),
            expr,
            body,
        }
    }

    /// Parses a `@match` expression.
    ///
    /// The leading `@match` should already be consumed.
    fn match_expr(&mut self, at_span: Span, keyword: TokenTree) -> ast::Markup {
        let keyword_span = keyword.span();
        let mut head = vec![keyword];
        let (arms, arms_span) = loop {
            match self.next() {
                Some(TokenTree::Group(ref body)) if body.delimiter() == Delimiter::Brace => {
                    let span = SpanRange::single_span(body.span());
                    break (self.with_input(body.stream()).match_arms(), span);
                }
                Some(token) => head.push(token),
                None => {
                    let span = SpanRange {
                        first: at_span,
                        last: keyword_span,
                    };
                    abort!(span, "expected body for this `@match`");
                }
            }
        };
        ast::Markup::Match {
            at_span: SpanRange::single_span(at_span),
            head: head.into_iter().collect(),
            arms,
            arms_span,
        }
    }

    fn match_arms(&mut self) -> Vec<ast::MatchArm> {
        let mut arms = Vec::new();
        while let Some(arm) = self.match_arm() {
            arms.push(arm);
        }
        arms
    }

    fn match_arm(&mut self) -> Option<ast::MatchArm> {
        let mut head = Vec::new();
        loop {
            match self.peek2() {
                Some((TokenTree::Punct(ref eq), Some(TokenTree::Punct(ref gt))))
                    if eq.as_char() == '='
                        && gt.as_char() == '>'
                        && eq.spacing() == Spacing::Joint =>
                {
                    head.push(TokenTree::Punct(eq.clone()));
                    head.push(TokenTree::Punct(gt.clone()));
                    self.advance2();
                    break;
                }
                Some((token, _)) => {
                    head.push(token.clone());
                    self.advance();
                }
                None => {
                    if head.is_empty() {
                        return None;
                    } else {
                        let head_span = ast::span_tokens(head);
                        abort!(head_span, "unexpected end of @match pattern");
                    }
                }
            }
        }
        let body = match self.next() {
            // $pat => { $stmts }
            Some(TokenTree::Group(ref body)) if body.delimiter() == Delimiter::Brace => {
                let body = self.block(body.stream(), SpanRange::single_span(body.span()));
                // Trailing commas are optional if the match arm is a braced block
                if let Some(TokenTree::Punct(ref punct)) = self.peek() {
                    if punct.as_char() == ',' {
                        self.advance();
                    }
                }
                body
            }
            // $pat => $expr
            Some(first_token) => {
                let mut span = SpanRange::single_span(first_token.span());
                let mut body = vec![first_token];
                loop {
                    match self.next() {
                        Some(TokenTree::Punct(ref punct)) if punct.as_char() == ',' => break,
                        Some(token) => {
                            span.last = token.span();
                            body.push(token);
                        }
                        None => break,
                    }
                }
                self.block(body.into_iter().collect(), span)
            }
            None => {
                let span = ast::span_tokens(head);
                abort!(span, "unexpected end of @match arm");
            }
        };
        Some(ast::MatchArm {
            head: head.into_iter().collect(),
            body,
        })
    }

    /// Parses a `@let` expression.
    ///
    /// The leading `@let` should already be consumed.
    fn let_expr(&mut self, at_span: Span, keyword: TokenTree) -> ast::Markup {
        let mut tokens = vec![keyword];
        loop {
            match self.next() {
                Some(token) => match token {
                    TokenTree::Punct(ref punct) if punct.as_char() == '=' => {
                        tokens.push(token.clone());
                        break;
                    }
                    _ => tokens.push(token),
                },
                None => {
                    let mut span = ast::span_tokens(tokens);
                    span.first = at_span;
                    abort!(span, "unexpected end of `@let` expression");
                }
            }
        }
        loop {
            match self.next() {
                Some(token) => match token {
                    TokenTree::Punct(ref punct) if punct.as_char() == ';' => {
                        tokens.push(token.clone());
                        break;
                    }
                    _ => tokens.push(token),
                },
                None => {
                    let mut span = ast::span_tokens(tokens);
                    span.first = at_span;
                    abort!(
                        span,
                        "unexpected end of `@let` expression";
                        help = "are you missing a semicolon?"
                    );
                }
            }
        }
        ast::Markup::Let {
            at_span: SpanRange::single_span(at_span),
            tokens: tokens.into_iter().collect(),
        }
    }

    /// Parses an element node.
    ///
    /// The element name should already be consumed.
    fn element(&mut self, name: TokenStream) -> ast::Markup {
        if self.current_attr.is_some() {
            let span = ast::span_tokens(name);
            abort!(span, "unexpected element");
        }
        let attrs = self.attrs();
        let body = match self.peek() {
            Some(TokenTree::Punct(ref punct))
                if punct.as_char() == ';' || punct.as_char() == '/' =>
            {
                // Void element
                let c = punct.as_char();
                let semi_span = SpanRange::single_span(punct.span());
                if c == '/' {
                    emit_error!(
                        punct,
                        "void elements must use `;`, not `/`";
                        help = "change this to `;`";
                        help = "see https://github.com/lambda-fairy/maud/pull/315 for details";
                    );
                }
                self.advance();
                ast::ElementBody::Void { semi_span }
            }
            Some(_) => match self.markup() {
                ast::Markup::Block(block) => ast::ElementBody::Block { block },
                markup => {
                    let markup_span = markup.span();
                    abort!(
                        markup_span,
                        "element body must be wrapped in braces";
                        help = "see https://github.com/lambda-fairy/maud/pull/137 for details"
                    );
                }
            },
            None => abort_call_site!("expected `;`, found end of macro"),
        };
        ast::Markup::Element { name, attrs, body }
    }

    /// Parses the attributes of an element.
    fn attrs(&mut self) -> Vec<ast::Attr> {
        let mut attrs = Vec::new();
        loop {
            if let Some(name) = self.try_namespaced_name() {
                // Attribute
                match self.peek() {
                    // Non-empty attribute
                    Some(TokenTree::Punct(ref punct)) if punct.as_char() == '=' => {
                        self.advance();
                        // Parse a value under an attribute context
                        assert!(self.current_attr.is_none());
                        self.current_attr = Some(ast::name_to_string(name.clone()));
                        let attr_type = match self.attr_toggler() {
                            Some(toggler) => ast::AttrType::Optional { toggler },
                            None => {
                                let value = self.markup();
                                ast::AttrType::Normal { value }
                            }
                        };
                        self.current_attr = None;
                        attrs.push(ast::Attr::Named {
                            named_attr: ast::NamedAttr { name, attr_type },
                        });
                    }
                    // Empty attribute (legacy syntax)
                    Some(TokenTree::Punct(ref punct)) if punct.as_char() == '?' => {
                        self.advance();
                        let toggler = self.attr_toggler();
                        attrs.push(ast::Attr::Named {
                            named_attr: ast::NamedAttr {
                                name: name.clone(),
                                attr_type: ast::AttrType::Empty { toggler },
                            },
                        });
                    }
                    // Empty attribute (new syntax)
                    _ => {
                        let toggler = self.attr_toggler();
                        attrs.push(ast::Attr::Named {
                            named_attr: ast::NamedAttr {
                                name: name.clone(),
                                attr_type: ast::AttrType::Empty { toggler },
                            },
                        });
                    }
                }
            } else {
                match self.peek() {
                    // Class shorthand
                    Some(TokenTree::Punct(ref punct)) if punct.as_char() == '.' => {
                        let dot_span = SpanRange::single_span(punct.span());
                        self.advance();
                        let name = self.class_or_id_name();
                        let toggler = self.attr_toggler();
                        attrs.push(ast::Attr::Class {
                            dot_span,
                            name,
                            toggler,
                        });
                    }
                    // ID shorthand
                    Some(TokenTree::Punct(punct)) if punct.as_char() == '#' => {
                        let hash_span = SpanRange::single_span(punct.span());
                        self.advance();
                        let name = self.class_or_id_name();
                        attrs.push(ast::Attr::Id { hash_span, name });
                    }
                    // If it's not a valid attribute, backtrack and bail out
                    _ => break,
                }
            }
        }

        let mut attr_map: HashMap<String, Vec<SpanRange>> = HashMap::new();
        let mut has_class = false;
        for attr in &attrs {
            let name = match attr {
                ast::Attr::Class { .. } => {
                    if has_class {
                        // Only check the first class to avoid spurious duplicates
                        continue;
                    }
                    has_class = true;
                    "class".to_string()
                }
                ast::Attr::Id { .. } => "id".to_string(),
                ast::Attr::Named { named_attr } => named_attr
                    .name
                    .clone()
                    .into_iter()
                    .map(|token| token.to_string())
                    .collect(),
            };
            let entry = attr_map.entry(name).or_default();
            entry.push(attr.span());
        }

        for (name, spans) in attr_map {
            if spans.len() > 1 {
                let mut spans = spans.into_iter();
                let first_span = spans.next().expect("spans should be non-empty");
                abort!(first_span, "duplicate attribute `{}`", name);
            }
        }

        attrs
    }

    /// Parses the name of a class or ID.
    fn class_or_id_name(&mut self) -> ast::Markup {
        if let Some(symbol) = self.try_name() {
            ast::Markup::Symbol { symbol }
        } else {
            self.markup()
        }
    }

    /// Parses the `[cond]` syntax after an empty attribute or class shorthand.
    fn attr_toggler(&mut self) -> Option<ast::Toggler> {
        match self.peek() {
            Some(TokenTree::Group(group)) if group.delimiter() == Delimiter::Bracket => {
                let cond = group.stream();
                let cond_span = SpanRange::single_span(group.span());
                self.advance();
                Some(ast::Toggler { cond, cond_span })
            }
            _ => None,
        }
    }

    /// Parses an identifier, without dealing with namespaces.
    fn try_name(&mut self) -> Option<TokenStream> {
        let mut result = Vec::new();
        if let Some(token @ TokenTree::Ident(_)) = self.peek() {
            result.push(token.clone());
            self.advance();
        } else {
            return None;
        }
        let mut expect_ident_or_num = false;
        loop {
            expect_ident_or_num = match self.peek() {
                Some(TokenTree::Punct(ref punct)) if punct.as_char() == '-' => {
                    result.push(TokenTree::Punct(punct.clone()));
                    self.advance();
                    true
                }
                Some(TokenTree::Ident(ref ident)) if expect_ident_or_num => {
                    result.push(TokenTree::Ident(ident.clone()));
                    self.advance();
                    false
                }
                // https://github.com/lambda-fairy/maud/pull/336/files
                Some(TokenTree::Literal(ref lit))
                    if expect_ident_or_num && lit.to_string().chars().all(|c| c.is_numeric()) =>
                {
                    result.push(TokenTree::Literal(lit.clone()));
                    self.advance();
                    false
                }
                _ => break,
            };
        }
        Some(result.into_iter().collect())
    }

    /// Parses a HTML element or attribute name, along with a namespace
    /// if necessary.
    fn try_namespaced_name(&mut self) -> Option<TokenStream> {
        let mut result = vec![self.try_name()?];
        if let Some(TokenTree::Punct(ref punct)) = self.peek() {
            if punct.as_char() == ':' {
                let punct = punct.clone();
                self.advance();
                result.push(TokenStream::from(TokenTree::Punct(punct)));
                result.push(self.try_name()?);
            }
        }
        Some(result.into_iter().collect())
    }

    /// Parses the given token stream as a Maud expression.
    fn block(&mut self, body: TokenStream, outer_span: SpanRange) -> ast::Block {
        let markups = self.with_input(body).markups();
        ast::Block {
            markups,
            outer_span,
        }
    }
}
