use toml_edit::{Document, value};

fn main() {
    // Create a new empty TOML document
    let mut doc = Document::new();

    // Add a table named "table"
    doc["table"] = toml_edit::table();

    // Add a key-value pair to the table
    doc["table"]["key"] = value("value");

    // Print the resulting TOML document
    println!("{}", doc.to_string());
}

