use serde_json::json;
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::SimpleFileOptions};

// Only public connection metadata enters the archive. The owner supplies
// credentials directly to the OAuth client and the pairing page.
pub fn write(origin: &str) -> Result<(), String> {
    let mcp = json!({
        "$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
        "mcpServers": {
            "glasses-inbox": {
                "type": "streamable-http",
                "url": format!("{origin}/mcp"),
                "extensions": {"com.openai": {"auth": {
                    "type": "oauth",
                    "client": {"mode":"provided","clientId":"g2-dot-link","tokenEndpointAuthMethod":"client_secret_basic"},
                    "baseScopes":["glasses"]
                }}}
            }
        }
    }).to_string();
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, body) in [
        ("plugin.json", include_str!("../plugin/plugin.json")),
        ("mcp.json", mcp.as_str()),
        (
            "skills/glasses-inbox/SKILL.md",
            include_str!("../plugin/skills/glasses-inbox/SKILL.md"),
        ),
    ] {
        zip.start_file(name, options)
            .and_then(|_| Ok(zip.write_all(body.as_bytes())?))
            .map_err(|_| "Cannot assemble plugin ZIP")?;
    }
    let bytes = zip
        .finish()
        .map_err(|_| "Cannot finish plugin ZIP")?
        .into_inner();
    let mut file = tempfile::NamedTempFile::new_in(".")
        .map_err(|_| "Cannot create plugin ZIP in this directory")?;
    file.write_all(&bytes)
        .map_err(|_| "Cannot write plugin ZIP")?;
    file.persist("g2-dot-link.zip")
        .map_err(|_| "Cannot install g2-dot-link.zip in this directory")?;
    println!("Upload g2-dot-link.zip in ChatGPT under Plugins → Add → Upload plugin.");
    Ok(())
}
