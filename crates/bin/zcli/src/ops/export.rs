use crate::address;
use crate::error::Error;
use crate::key::WalletSeed;

pub fn export(
    seed: &WalletSeed,
    mainnet: bool,
    script: bool,
) -> Result<(), Error> {
    // interactive confirmation unless script mode or non-tty
    if !script && is_terminal::is_terminal(std::io::stdin()) {
        eprintln!("WARNING: this will display secret key material.");
        eprintln!("anyone with these keys can spend your funds.");
        eprint!("type YES to continue: ");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)
            .map_err(|e| Error::Io(e))?;
        if input.trim() != "YES" {
            return Err(Error::Other("export cancelled".into()));
        }
    }

    let fvk = address::full_viewing_key(seed, mainnet)?;
    let fvk_bytes = fvk.to_bytes();

    let taddr = address::transparent_address(seed, mainnet)?;
    let uaddr = address::orchard_address(seed, mainnet)?;

    if script {
        println!("{}", serde_json::json!({
            "full_viewing_key": hex::encode(fvk_bytes),
            "transparent_address": taddr,
            "unified_address": uaddr,
        }));
    } else {
        println!("full viewing key: {}", hex::encode(fvk_bytes));
        println!("transparent:      {}", taddr);
        println!("unified:          {}", uaddr);
    }

    Ok(())
}
