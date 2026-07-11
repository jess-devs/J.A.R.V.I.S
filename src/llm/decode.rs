//! Decodificador UTF-8 incremental para streams de bytes HTTP.
//!
//! Los chunks TCP pueden cortar un carácter multibyte por la mitad (á, ñ,
//! ¿… son 2 bytes en UTF-8). Decodificar cada chunk por separado con
//! `String::from_utf8_lossy` convierte ese carácter partido en U+FFFD y el
//! TTS lo pronuncia mal. Este decoder retiene el sufijo incompleto y lo
//! completa con el chunk siguiente.

pub(crate) struct Utf8StreamDecoder {
    /// Sufijo del último chunk que quedó a mitad de un carácter (<=3 bytes).
    pending: Vec<u8>,
}

impl Utf8StreamDecoder {
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
        }
    }

    /// Añade `bytes` y vuelca en `out` todo el texto decodificable hasta el
    /// último límite UTF-8 válido. Los bytes realmente inválidos (no una
    /// secuencia incompleta al final) se reemplazan por U+FFFD, igual que
    /// `from_utf8_lossy`.
    pub fn feed(&mut self, bytes: &[u8], out: &mut String) {
        self.pending.extend_from_slice(bytes);
        let mut start = 0;
        loop {
            match std::str::from_utf8(&self.pending[start..]) {
                Ok(s) => {
                    out.push_str(s);
                    start = self.pending.len();
                    break;
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    out.push_str(
                        std::str::from_utf8(&self.pending[start..start + valid])
                            .expect("from_utf8 ya validó este prefijo"),
                    );
                    match e.error_len() {
                        // Bytes inválidos de verdad: reemplazar y seguir.
                        Some(len) => {
                            out.push(char::REPLACEMENT_CHARACTER);
                            start += valid + len;
                        }
                        // Secuencia incompleta al final: esperar más bytes.
                        None => {
                            start += valid;
                            break;
                        }
                    }
                }
            }
        }
        self.pending.drain(..start);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_in_chunks(chunks: &[&[u8]]) -> String {
        let mut decoder = Utf8StreamDecoder::new();
        let mut out = String::new();
        for chunk in chunks {
            decoder.feed(chunk, &mut out);
        }
        out
    }

    #[test]
    fn ascii_pasa_directo() {
        let mut decoder = Utf8StreamDecoder::new();
        let mut out = String::new();
        decoder.feed(b"hola mundo", &mut out);
        assert_eq!(out, "hola mundo");
        assert!(decoder.pending.is_empty());
    }

    #[test]
    fn multibyte_partido_entre_chunks() {
        // "¿mañana?": ¿ = C2 BF, ñ = C3 B1 (índices 4..6). Cortar en 5
        // parte la ñ justo por la mitad.
        let bytes = "¿mañana?".as_bytes();
        assert_eq!(bytes[4], 0xC3);
        let out = decode_in_chunks(&[&bytes[..5], &bytes[5..]]);
        assert_eq!(out, "¿mañana?");
    }

    #[test]
    fn multibyte_byte_a_byte() {
        let bytes = "¿mañana? sí, a las 3".as_bytes();
        let chunks: Vec<&[u8]> = bytes.chunks(1).collect();
        assert_eq!(decode_in_chunks(&chunks), "¿mañana? sí, a las 3");
    }

    #[test]
    fn emoji_de_4_bytes_partido() {
        let bytes = "ok 🎉!".as_bytes();
        // El emoji ocupa los bytes 3..7: partir en 2+2.
        let out = decode_in_chunks(&[&bytes[..5], &bytes[5..]]);
        assert_eq!(out, "ok 🎉!");
    }

    #[test]
    fn bytes_invalidos_se_reemplazan() {
        let out = decode_in_chunks(&[b"a\xFFb"]);
        assert_eq!(out, "a\u{FFFD}b");
    }
}
