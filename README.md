# maud 

this is a fork of [maud](https://github.com/lambda-fairy/maud)

it adds custom components, an integration with [zephyr](https://github.com/annieversary/zephyr)
and other minor improvements.
i also removed stuff i don't care about, like the integrations with web frameworks and the docs.
it no longer supports `no_std`

you should probably use upstream instead

## custom components

a custom component is any function that returns a `Markup` 

if the component takes a `Markup` body as an argument, it has to be the last one:

### with body

if you define a function like:

```rust
fn my_component(title: &str, children: Markup) -> Markup {
    html! {
        p { (title) }
        div {
            (children)
        }
    }
}
```

you can use it like:

```rust
@my_component("hey") {
    p { "hello" }
}

// it's equivalent to:
(my_component("hey", html! {
    p { "hello" }
}))
```

### without body

a body is not required:

```rust
fn my_component(title: &str) -> Markup {
    html! {
        p { (title) }
    }
}
```

you can use it like:

```rust
@my_component("hey");

// it's equivalent to:
(my_component("hey"))
```

## zephyr

enabling the `zephyr` feature will register all the used classes to the `zephyr` inventory.
you don't have to do anything else maud related :)

this then allows you to write html using the `html!` macro,
and then to generate the corresponding css using something like

```rust
pub async fn generate_css_from_inventory(path: impl AsRef<Path>) -> std::io::Result<()> {
    let z = maud::zephyr::Zephyr::new();
    let generated_css = z.generate_from_inventory();

    let mut file = File::create(path)?;
    file.write_all(generated_css.as_bytes())?;

    Ok(())
}
```

this only works for literals, eg. `div.m[1rem]` or `div class="m[1rem]"`. it does not work for the following:

```rust
@let c = "m[1rem]";
div class=(c) {
    [...]
}
```

## original readme

[Documentation][book] ([source][booksrc]) •
[API reference][apiref] •
[Change log][changelog]

Maud is an HTML template engine for Rust.
It's implemented as a macro, `html!`,
which compiles your markup to specialized Rust code.
This unique approach makes Maud templates
blazing fast, super type-safe, and easy to deploy.

For more info on Maud,
see the [official book][book].

[book]: https://maud.lambda.xyz/
[booksrc]: https://github.com/lambda-fairy/maud/tree/main/docs
[apiref]: https://docs.rs/maud/
[changelog]: https://github.com/lambda-fairy/maud/blob/main/CHANGELOG.md
