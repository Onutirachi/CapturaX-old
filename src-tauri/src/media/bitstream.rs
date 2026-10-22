//! Conversão de vídeo em Annex B (códigos de início 00 00 01) para o formato do MP4
//! (cada NAL precedido do tamanho), e montagem do `avcC`/`hvcC` a partir dos parâmetros.
//!
//! Necessário porque o encoder HEVC do Chromium entrega Annex B com VPS/SPS/PPS dentro do
//! vídeo e sem `decoderConfig.description`, mesmo pedindo o formato "hevc".

use super::fmp4::{VideoCodec, VideoColor};

pub fn is_annexb(data: &[u8]) -> bool {
    data.starts_with(&[0, 0, 0, 1]) || data.starts_with(&[0, 0, 1])
}

/// Separa as NAL units de um pacote Annex B.
pub fn split_annexb(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut nals = Vec::with_capacity(starts.len());
    for (k, &start) in starts.iter().enumerate() {
        let mut end = if k + 1 < starts.len() { starts[k + 1] - 3 } else { data.len() };
        // O zero extra de um código de 4 bytes (00 00 00 01) pertence ao próximo início.
        while end > start && data[end - 1] == 0 && k + 1 < starts.len() {
            end -= 1;
        }
        if end > start {
            nals.push(&data[start..end]);
        }
    }
    nals
}

fn nal_type(codec: VideoCodec, nal: &[u8]) -> u8 {
    match codec {
        VideoCodec::Avc => nal[0] & 0x1f,
        VideoCodec::Hevc => (nal[0] >> 1) & 0x3f,
    }
}

/// Parâmetros de codec (vão no `avcC`/`hvcC`) e delimitadores de acesso (descartáveis).
fn is_parameter_or_delimiter(codec: VideoCodec, nal: &[u8]) -> bool {
    let kind = nal_type(codec, nal);
    match codec {
        VideoCodec::Avc => matches!(kind, 7 | 8 | 9),
        VideoCodec::Hevc => matches!(kind, 32..=35),
    }
}

/// Pacote Annex B → NALs com prefixo de 4 bytes, sem parâmetros nem delimitadores.
pub fn to_length_prefixed(codec: VideoCodec, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    for nal in split_annexb(data) {
        if is_parameter_or_delimiter(codec, nal) {
            continue;
        }
        out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
        out.extend_from_slice(nal);
    }
    out
}

/// Monta o `avcC`/`hvcC` com os parâmetros encontrados num keyframe Annex B.
pub fn build_description(codec: VideoCodec, keyframe: &[u8]) -> Option<Vec<u8>> {
    let nals = split_annexb(keyframe);
    let find = |kind: u8| nals.iter().find(|n| !n.is_empty() && nal_type(codec, n) == kind).copied();
    match codec {
        VideoCodec::Avc => build_avcc(find(7)?, find(8)?),
        VideoCodec::Hevc => build_hvcc(find(32)?, find(33)?, find(34)?),
    }
}

fn build_avcc(sps: &[u8], pps: &[u8]) -> Option<Vec<u8>> {
    if sps.len() < 4 {
        return None;
    }
    let mut out = vec![1, sps[1], sps[2], sps[3], 0xff, 0xe1];
    out.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    out.extend_from_slice(sps);
    out.push(1);
    out.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    out.extend_from_slice(pps);
    Some(out)
}

/// Remove os bytes de prevenção de emulação (00 00 03 → 00 00).
fn unescape(nal: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal.len());
    let mut zeros = 0;
    for &byte in nal {
        if zeros >= 2 && byte == 3 {
            zeros = 0;
            continue;
        }
        zeros = if byte == 0 { zeros + 1 } else { 0 };
        out.push(byte);
    }
    out
}

fn build_hvcc(vps: &[u8], sps: &[u8], pps: &[u8]) -> Option<Vec<u8>> {
    let rbsp = unescape(sps);
    // Cabeçalho NAL (2 bytes) + 1 byte (vps id, max_sub_layers_minus1, temporal_id_nesting)
    // + profile_tier_level geral (12 bytes), que vai igual para o hvcC.
    if rbsp.len() < 15 {
        return None;
    }
    let max_sub_layers_minus1 = (rbsp[2] >> 1) & 0x07;
    let temporal_id_nested = rbsp[2] & 0x01;
    let ptl = &rbsp[3..15];
    let profile_idc = ptl[0] & 0x1f;
    // Main 10 (2) e Main Still Picture… só o Main 10 muda a profundidade de bits.
    let bit_depth_minus8: u8 = if profile_idc == 2 { 2 } else { 0 };

    let mut out = vec![1];
    out.extend_from_slice(ptl);
    out.extend_from_slice(&[0xf0, 0x00]); // min_spatial_segmentation_idc = 0
    out.push(0xfc); // parallelismType = 0
    out.push(0xfc | 1); // chroma 4:2:0
    out.push(0xf8 | bit_depth_minus8);
    out.push(0xf8 | bit_depth_minus8);
    out.extend_from_slice(&[0, 0]); // avgFrameRate
    out.push(((max_sub_layers_minus1 + 1) << 3) | (temporal_id_nested << 2) | 3); // NALs com 4 bytes de tamanho
    out.push(3);
    for (kind, nal) in [(32u8, vps), (33, sps), (34, pps)] {
        out.push(0x80 | kind);
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(&(nal.len() as u16).to_be_bytes());
        out.extend_from_slice(nal);
    }
    Some(out)
}

/// Coloca a cor (faixa completa/limitada, primárias, transferência, matriz) no SPS do `avcC`.
///
/// O encoder da GPU recebe os frames da placa em faixa completa (MJPEG) e codifica assim, mas não
/// escreve isso no SPS: quem toca supõe faixa limitada e a imagem fica escura e contrastada.
/// Devolve `None` se o `avcC` não for o esperado (fica como veio).
pub fn set_avc_color(avcc: &[u8], color: VideoColor) -> Option<Vec<u8>> {
    if avcc.len() < 8 || avcc[0] != 1 || avcc[5] & 0x1f != 1 {
        return None;
    }
    let sps_len = u16::from_be_bytes([avcc[6], avcc[7]]) as usize;
    let sps = avcc.get(8..8 + sps_len)?;
    let patched = escape(&patch_sps_rbsp(&unescape(sps.get(1..)?), color)?);
    let mut out = avcc[..6].to_vec();
    out.extend_from_slice(&((patched.len() + 1) as u16).to_be_bytes());
    out.push(sps[0]);
    out.extend_from_slice(&patched);
    out.extend_from_slice(&avcc[8 + sps_len..]);
    Some(out)
}

/// Recoloca os bytes de prevenção de emulação (00 00 0x → 00 00 03 0x, x ≤ 3).
fn escape(rbsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rbsp.len() + 4);
    let mut zeros = 0;
    for &byte in rbsp {
        if zeros >= 2 && byte <= 3 {
            out.push(3);
            zeros = 0;
        }
        zeros = if byte == 0 { zeros + 1 } else { 0 };
        out.push(byte);
    }
    out
}

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl BitReader<'_> {
    fn bit(&mut self) -> Option<u32> {
        let byte = *self.data.get(self.pos / 8)?;
        let bit = (byte >> (7 - self.pos % 8)) & 1;
        self.pos += 1;
        Some(bit as u32)
    }
    fn bits(&mut self, n: usize) -> Option<u32> {
        (0..n).try_fold(0, |value, _| Some((value << 1) | self.bit()?))
    }
    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        Some((1u32 << zeros) - 1 + self.bits(zeros)?)
    }
    fn se(&mut self) -> Option<i32> {
        let code = self.ue()?;
        Some(if code % 2 == 1 { code.div_ceil(2) as i32 } else { -((code / 2) as i32) })
    }
}

#[derive(Default)]
struct BitWriter {
    data: Vec<u8>,
    len: usize,
}

impl BitWriter {
    fn bit(&mut self, bit: u32) {
        if self.len % 8 == 0 {
            self.data.push(0);
        }
        if bit != 0 {
            *self.data.last_mut().unwrap() |= 1 << (7 - self.len % 8);
        }
        self.len += 1;
    }
    fn bits(&mut self, value: u32, n: usize) {
        for i in (0..n).rev() {
            self.bit((value >> i) & 1);
        }
    }
    /// Copia os bits `from..to` de `data`.
    fn copy(&mut self, data: &[u8], from: usize, to: usize) {
        for pos in from..to {
            self.bit(((data[pos / 8] >> (7 - pos % 8)) & 1) as u32);
        }
    }
}

fn skip_scaling_list(reader: &mut BitReader, size: usize) -> Option<()> {
    let (mut last, mut next) = (8i32, 8i32);
    for _ in 0..size {
        if next != 0 {
            next = (last + reader.se()? + 256) % 256;
        }
        last = if next == 0 { last } else { next };
    }
    Some(())
}

/// Reescreve o RBSP do SPS (sem o cabeçalho NAL) com `video_signal_type` = `color`.
fn patch_sps_rbsp(rbsp: &[u8], color: VideoColor) -> Option<Vec<u8>> {
    // Fim dos dados: o último bit 1 é o `rbsp_stop_one_bit`.
    let last = rbsp.iter().rposition(|&b| b != 0)?;
    let end = last * 8 + 7 - rbsp[last].trailing_zeros() as usize;

    let mut r = BitReader { data: rbsp, pos: 0 };
    let profile = r.bits(8)?;
    r.bits(16)?; // restrições + nível
    r.ue()?; // seq_parameter_set_id
    if matches!(profile, 100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135) {
        let chroma_format = r.ue()?;
        if chroma_format == 3 {
            r.bit()?;
        }
        r.ue()?; // profundidade de bits (luma, croma)
        r.ue()?;
        r.bit()?;
        if r.bit()? == 1 {
            for i in 0..if chroma_format == 3 { 12 } else { 8 } {
                if r.bit()? == 1 {
                    skip_scaling_list(&mut r, if i < 6 { 16 } else { 64 })?;
                }
            }
        }
    }
    r.ue()?; // log2_max_frame_num_minus4
    match r.ue()? {
        0 => {
            r.ue()?;
        }
        1 => {
            r.bit()?;
            r.se()?;
            r.se()?;
            for _ in 0..r.ue()? {
                r.se()?;
            }
        }
        _ => {}
    }
    r.ue()?; // max_num_ref_frames
    r.bit()?;
    r.ue()?; // tamanho em macroblocos
    r.ue()?;
    if r.bit()? == 0 {
        r.bit()?;
    }
    r.bit()?;
    if r.bit()? == 1 {
        for _ in 0..4 {
            r.ue()?;
        }
    }

    let mut w = BitWriter::default();
    w.copy(rbsp, 0, r.pos);
    let had_vui = r.bit()? == 1;
    w.bit(1);
    let signal = |w: &mut BitWriter| {
        w.bit(1); // video_signal_type_present_flag
        w.bits(5, 3); // video_format: não especificado
        w.bit(color.full_range as u32);
        w.bit(1); // colour_description_present_flag
        w.bits(color.primaries as u32, 8);
        w.bits(color.transfer as u32, 8);
        w.bits(color.matrix as u32, 8);
    };
    if had_vui {
        let start = r.pos;
        if r.bit()? == 1 && r.bits(8)? == 255 {
            r.bits(32)?;
        }
        if r.bit()? == 1 {
            r.bit()?;
        }
        w.copy(rbsp, start, r.pos);
        if r.bit()? == 1 {
            r.bits(4)?;
            if r.bit()? == 1 {
                r.bits(24)?;
            }
        }
        signal(&mut w);
        if r.pos > end {
            return None;
        }
        w.copy(rbsp, r.pos, end);
    } else {
        if r.pos - 1 != end {
            return None;
        }
        w.bit(0); // aspect_ratio_info_present_flag
        w.bit(0); // overscan_info_present_flag
        signal(&mut w);
        w.bits(0, 5); // chroma_loc, timing, nal_hrd, vcl_hrd, pic_struct
        w.bit(0); // bitstream_restriction_flag
    }
    w.bit(1);
    while w.len % 8 != 0 {
        w.bit(0);
    }
    Some(w.data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_annexb_and_builds_avcc() {
        let sps = [0x67, 0x64, 0x00, 0x1f, 0xac];
        let pps = [0x68, 0xee, 0x3c];
        let idr = [0x65, 0x88, 0x00, 0x00, 0x00, 0x10];
        let mut packet = vec![0, 0, 0, 1, 0x09, 0xf0];
        for nal in [&sps[..], &pps[..], &idr[..]] {
            packet.extend_from_slice(&[0, 0, 0, 1]);
            packet.extend_from_slice(nal);
        }
        assert!(is_annexb(&packet));
        assert_eq!(split_annexb(&packet).len(), 4);
        let converted = to_length_prefixed(VideoCodec::Avc, &packet);
        assert_eq!(converted, [&[0, 0, 0, 6][..], &idr[..]].concat());
        let avcc = build_description(VideoCodec::Avc, &packet).unwrap();
        assert_eq!(&avcc[..4], &[1, 0x64, 0x00, 0x1f]);
    }

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap()).collect()
    }

    /// Lê de volta `video_signal_type` e os bits depois dele (têm que ficar iguais).
    fn read_signal(avcc: &[u8]) -> (Option<(u32, u32, u32, u32)>, Vec<u32>) {
        let len = u16::from_be_bytes([avcc[6], avcc[7]]) as usize;
        let rbsp = unescape(&avcc[9..8 + len]);
        // Os SPS de teste: High 4:2:0 sem matriz de escala, poc tipo 2.
        let mut r = BitReader { data: &rbsp, pos: 0 };
        r.bits(24);
        for _ in 0..4 {
            r.ue();
        }
        r.bits(2);
        r.ue();
        assert_eq!(r.ue(), Some(2), "poc tipo 2");
        r.ue();
        r.bit();
        r.ue();
        r.ue();
        if r.bit() == Some(0) {
            r.bit();
        }
        r.bit();
        if r.bit() == Some(1) {
            for _ in 0..4 {
                r.ue();
            }
        }
        assert_eq!(r.bit(), Some(1), "VUI");
        if r.bit() == Some(1) && r.bits(8) == Some(255) {
            r.bits(32);
        }
        if r.bit() == Some(1) {
            r.bit();
        }
        let signal = if r.bit() == Some(1) {
            r.bits(3);
            let full = r.bit().unwrap();
            assert_eq!(r.bit(), Some(1));
            Some((full, r.bits(8).unwrap(), r.bits(8).unwrap(), r.bits(8).unwrap()))
        } else {
            None
        };
        let rest = std::iter::from_fn(|| r.bit()).collect::<Vec<_>>();
        // Sem o alinhamento final (muda com o tamanho).
        let stop = rest.iter().rposition(|&b| b == 1).unwrap();
        (signal, rest[..stop].to_vec())
    }

    #[test]
    fn sets_avc_color() {
        let color = VideoColor { primaries: 1, transfer: 1, matrix: 6, full_range: true };
        // Do encoder da GPU (1080p e 320x240), com VUI só de tempo.
        for text in ["01640028ffe1001967640028ac2b403c0113f2e022000007d00001d4c01e38554001000468ee3cb0fdf8f800", "0164001fffe100166764001fac2b40a0fd808800001f400007530078e15501000468ee3cb0fdf8f800"] {
            let avcc = hex(text);
            let (before, rest_before) = read_signal(&avcc);
            assert_eq!(before, None);
            let patched = set_avc_color(&avcc, color).unwrap();
            let (after, rest_after) = read_signal(&patched);
            assert_eq!(after, Some((1, 1, 1, 6)));
            assert_eq!(rest_before, rest_after);
            // PPS e o resto do avcC intactos.
            assert!(patched.ends_with(&avcc[8 + u16::from_be_bytes([avcc[6], avcc[7]]) as usize..]));
            // Aplicar de novo troca a cor sem acumular nada.
            let again = set_avc_color(&patched, VideoColor { full_range: false, ..color }).unwrap();
            assert_eq!(read_signal(&again).0, Some((0, 1, 1, 6)));
            assert_eq!(again.len(), patched.len());
        }
    }

    #[test]
    fn unescapes_emulation_prevention() {
        assert_eq!(unescape(&[0, 0, 3, 1, 0, 0, 3, 0]), vec![0, 0, 1, 0, 0, 0]);
    }
}
