# daggerfall-data

Lettori dei formati di dati di Daggerfall in Rust, senza dipendenze, portati da
Daggerfall Unity (MIT, vedi NOTICE).

Per ora: mappa del mondo (`pak` CLIMATE/POLITIC.PAK, `woods` WOODS.WLD), altezze del terreno (`heightmap`, `perlin`), selezione per clima/stagione (`climate`), tile del terreno (`terrain`), tilemap (`tilemap`), `TEXTURE.nnn` (tutte le compressioni, archivi a colore pieno 000/001) e
palette `.PAL`/`.COL`. L'output delle texture sono **indici di palette a 8 bit**
(R8), nell'ordine del file (prima riga in alto), pronti per un lookup nello shader.

## Uso rapido

    cargo test
    cargo run --release --bin dump_texture -- /percorso/ARENA2 112
    cargo run --release --bin dump_texture -- /percorso/ARENA2 112 --out /tmp/png --alpha0

Senza `--out` stampa solo record, dimensioni, compressione e il CRC32 degli
indici di ogni frame. Con `--record N` / `--frame N` si limita l'output.

## Dal codice

    let tex = daggerfall_data::TextureFile::open("ARENA2/TEXTURE.112")?;
    let img = tex.frame(0, 0)?;           // IndexedImage { width, height, indices }
    let pal = daggerfall_data::Palette::open("ARENA2/ART_PAL.COL")?;
    let rgba = img.to_rgba(&pal, Some(0)); // indice 0 trasparente

## Clima e terreno

    use daggerfall_data::*;
    // archivio di un edificio per clima e stagione (stesse regole e stranezze di DFU)
    let a = apply_climate(12, 0, ClimateBase::Temperate, Season::Winter); // 313
    // tile del terreno per il clima mondiale 231 (boschi), d'inverno -> TEXTURE.303
    let ground = ground_archive(231, true);
    let tiles = load_tile_array(&TextureFile::open(format!("ARENA2/TEXTURE.{ground:03}"))?)?;
    let layer0: &[u8] = tiles.layer(0); // indici R8 della tile 0

    cargo run --release --bin dump_terrain -- /percorso/ARENA2 231 --out /tmp/terreno.png
    cargo run --release --bin dump_terrain -- /percorso/ARENA2 231 --winter --out /tmp/neve.png

## Mondo e altezze

    let woods = WoodsFile::open("ARENA2/WOODS.WLD")?;
    let climate = PakFile::open("ARENA2/CLIMATE.PAK")?;
    let world_climate = climate.map_pixel_value(300, 250).unwrap(); // 223..=232
    let samples = generate_default_samples(&woods, 300, 250)?;       // 129x129, 0..1
    let metri = samples.height(64, 64);                               // unità di DFU

    cargo run --release --bin dump_world -- /percorso/ARENA2 \
        --heightmap /tmp/altezze.png --climate /tmp/climi.png --samples 300 250 /tmp/pixel.png

Nota: il microrilievo usa un Perlin classico, non `Mathf.PerlinNoise` di Unity
(vedi `perlin.rs`); passare un'altra `NoiseFn` a `generate_samples` per sostituirlo.

## Tilemap del terreno

    let samples = generate_default_samples(&woods, 300, 250)?;
    let tilemap = generate_default_tilemap(&samples, 300, 250, None)?; // 128x128, indice x + y*128
    let per_shader = encode_for_shader(&tilemap, true);                // (tile<<2)|rotate|flip<<1

I valori della tilemap sono `indice tile (0..55) | 0x40 (rotazione) | 0x80 (specchiatura)`.
`preset` permette di passare le celle già fissate da una località (0 = libera, 0xFF =
acqua da non sovrascrivere).

    cargo run --release --bin dump_tilemap -- /percorso/ARENA2 300 250 --out /tmp/tilemap.png
    cargo run --release --bin dump_tilemap -- /percorso/ARENA2 300 250 --winter --scale 6 --out /tmp/neve.png

Il PNG colora ogni cella con il colore medio della sua tile (senza rotazioni): serve a
controllare dove DFU metterebbe acqua, spiaggia, terra, erba e pietra.

Due funzioni sostituibili perché non verificate bit a bit contro Unity: il rumore
(`NoiseFn`, vedi `perlin.rs`) e la variazione della linea di spiaggia (`BeachJitterFn`,
`tilemap::unity_beach_jitter`).
