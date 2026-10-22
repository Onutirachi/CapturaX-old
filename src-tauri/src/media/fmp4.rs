//! Muxer MP4 fragmentado (ISO BMFF) mínimo: H.264/HEVC + AAC.
//!
//! O mesmo formato serve para a gravação (arquivo que não corrompe se o app fechar no meio) e
//! para os segmentos HLS da transmissão. Ao terminar a gravação, `progressive_moov` gera o índice
//! completo para o arquivo virar um MP4 comum (ver `media::file`).

pub const VIDEO_TIMESCALE: u32 = 90_000;
pub const VIDEO_TRACK: u32 = 1;
/// Primeira faixa de áudio; as outras (trilhas separadas) vêm em seguida.
pub const AUDIO_TRACK: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VideoCodec {
    Avc,
    Hevc,
}

#[derive(Clone, Debug)]
pub struct VideoTrack {
    pub codec: VideoCodec,
    /// `avcC`/`hvcC` vindo do `decoderConfig.description` do WebCodecs.
    pub description: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Codec no formato RFC 6381 (`avc1.640033`), para o MediaSource de quem assiste.
    pub codec_string: String,
    /// Cor do vídeo como o encoder informou (vai no `colr` e, no H.264, no SPS).
    pub color: Option<VideoColor>,
}

/// Códigos da ISO/IEC 23091-2 (os mesmos do H.264/HEVC e do `colr`).
#[derive(Clone, Copy, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoColor {
    pub primaries: u8,
    pub transfer: u8,
    pub matrix: u8,
    pub full_range: bool,
}

#[derive(Clone, Debug)]
pub struct AudioTrack {
    /// Nome que os editores de vídeo mostram (`hdlr`).
    pub name: String,
    pub config: Vec<u8>,
    pub sample_rate: u32,
    pub channels: u16,
    pub bitrate: u32,
}

#[derive(Default)]
struct Writer {
    buf: Vec<u8>,
    open: Vec<usize>,
}

impl Writer {
    fn begin(&mut self, kind: &[u8; 4]) {
        self.open.push(self.buf.len());
        self.u32(0);
        self.bytes(kind);
    }

    fn begin_full(&mut self, kind: &[u8; 4], version: u8, flags: u32) {
        self.begin(kind);
        self.u32((version as u32) << 24 | flags);
    }

    fn end(&mut self) {
        let start = self.open.pop().expect("unbalanced box");
        let size = (self.buf.len() - start) as u32;
        self.buf[start..start + 4].copy_from_slice(&size.to_be_bytes());
    }

    fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }
    fn bytes(&mut self, v: &[u8]) {
        self.buf.extend_from_slice(v);
    }
    fn zeros(&mut self, n: usize) {
        self.buf.resize(self.buf.len() + n, 0);
    }
    fn matrix(&mut self) {
        for v in [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000u32] {
            self.u32(v);
        }
    }
}

fn ftyp(w: &mut Writer) {
    w.begin(b"ftyp");
    w.bytes(b"isom");
    w.u32(0x200);
    for brand in [b"isom", b"iso6", b"mp41", b"avc1"] {
        w.bytes(brand);
    }
    w.end();
}

fn mvhd(w: &mut Writer, duration_ms: u64, next_track: u32) {
    w.begin_full(b"mvhd", 1, 0);
    w.u64(0);
    w.u64(0);
    w.u32(1000);
    w.u64(duration_ms);
    w.u32(0x0001_0000);
    w.u16(0x0100);
    w.zeros(10);
    w.matrix();
    w.zeros(24);
    w.u32(next_track);
    w.end();
}

/// `alternate_group`: faixas de áudio do mesmo grupo são alternativas (o player toca uma, a
/// habilitada); editores importam todas.
#[allow(clippy::too_many_arguments)]
fn tkhd(w: &mut Writer, track: u32, duration_ms: u64, audio: bool, enabled: bool, alternate_group: u16, width: u32, height: u32) {
    w.begin_full(b"tkhd", 1, if enabled { 0x3 } else { 0x2 });
    w.u64(0);
    w.u64(0);
    w.u32(track);
    w.u32(0);
    w.u64(duration_ms);
    w.zeros(8);
    w.u16(0);
    w.u16(alternate_group);
    w.u16(if audio { 0x0100 } else { 0 });
    w.u16(0);
    w.matrix();
    w.u32(width << 16);
    w.u32(height << 16);
    w.end();
}

fn mdhd(w: &mut Writer, timescale: u32, duration: u64) {
    w.begin_full(b"mdhd", 1, 0);
    w.u64(0);
    w.u64(0);
    w.u32(timescale);
    w.u64(duration);
    w.u16(0x55c4); // "und"
    w.u16(0);
    w.end();
}

fn hdlr(w: &mut Writer, handler: &[u8; 4], name: &str) {
    w.begin_full(b"hdlr", 0, 0);
    w.u32(0);
    w.bytes(handler);
    w.zeros(12);
    w.bytes(name.as_bytes());
    w.u8(0);
    w.end();
}

fn dinf(w: &mut Writer) {
    w.begin(b"dinf");
    w.begin_full(b"dref", 0, 0);
    w.u32(1);
    w.begin_full(b"url ", 0, 1);
    w.end();
    w.end();
    w.end();
}

fn video_sample_entry(w: &mut Writer, video: &VideoTrack) {
    let (entry, config) = match video.codec {
        VideoCodec::Avc => (b"avc1", b"avcC"),
        VideoCodec::Hevc => (b"hvc1", b"hvcC"),
    };
    w.begin(entry);
    w.zeros(6);
    w.u16(1);
    w.zeros(16);
    w.u16(video.width as u16);
    w.u16(video.height as u16);
    w.u32(0x0048_0000);
    w.u32(0x0048_0000);
    w.u32(0);
    w.u16(1);
    w.zeros(32);
    w.u16(0x0018);
    w.u16(0xffff);
    w.begin(config);
    w.bytes(&video.description);
    w.end();
    if let Some(color) = video.color {
        w.begin(b"colr");
        w.bytes(b"nclx");
        w.u16(color.primaries as u16);
        w.u16(color.transfer as u16);
        w.u16(color.matrix as u16);
        w.u8(if color.full_range { 0x80 } else { 0 });
        w.end();
    }
    w.end();
}

fn descriptor(w: &mut Writer, tag: u8, body: &[u8]) {
    w.u8(tag);
    // Tamanho em 4 bytes (formato "expandido"), aceito por todos os leitores.
    let len = body.len() as u32;
    w.bytes(&[0x80 | (len >> 21) as u8 & 0x7f, 0x80 | (len >> 14) as u8 & 0x7f, 0x80 | (len >> 7) as u8 & 0x7f, len as u8 & 0x7f]);
    w.bytes(body);
}

fn audio_sample_entry(w: &mut Writer, audio: &AudioTrack) {
    w.begin(b"mp4a");
    w.zeros(6);
    w.u16(1);
    w.zeros(8);
    w.u16(audio.channels);
    w.u16(16);
    w.u16(0);
    w.u16(0);
    w.u32(audio.sample_rate << 16);

    let mut specific = Writer::default();
    descriptor(&mut specific, 0x05, &audio.config);
    let mut decoder = Writer::default();
    decoder.u8(0x40); // MPEG-4 Audio
    decoder.u8(0x15); // AudioStream
    decoder.bytes(&[0, 0, 0]);
    decoder.u32(audio.bitrate);
    decoder.u32(audio.bitrate);
    decoder.bytes(&specific.buf);
    let mut es = Writer::default();
    es.u16(0);
    es.u8(0);
    descriptor(&mut es, 0x04, &decoder.buf);
    descriptor(&mut es, 0x06, &[0x02]);

    w.begin_full(b"esds", 0, 0);
    descriptor(w, 0x03, &es.buf);
    w.end();
    w.end();
}

fn stsd(w: &mut Writer, video: Option<&VideoTrack>, audio: Option<&AudioTrack>) {
    w.begin_full(b"stsd", 0, 0);
    w.u32(1);
    if let Some(video) = video {
        video_sample_entry(w, video);
    }
    if let Some(audio) = audio {
        audio_sample_entry(w, audio);
    }
    w.end();
}

fn empty_tables(w: &mut Writer) {
    for kind in [b"stts", b"stsc", b"stco"] {
        w.begin_full(kind, 0, 0);
        w.u32(0);
        w.end();
    }
    w.begin_full(b"stsz", 0, 0);
    w.u32(0);
    w.u32(0);
    w.end();
}

/// Amostra já gravada, para montar o índice do MP4 final.
#[derive(Clone, Copy, Debug)]
pub struct IndexedSample {
    /// Posição absoluta no arquivo.
    pub offset: u64,
    pub size: u32,
    pub duration: u32,
    pub sync: bool,
}

fn sample_tables(w: &mut Writer, samples: &[IndexedSample], with_sync: bool) {
    // stts: durações em sequência (run-length).
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for s in samples {
        match runs.last_mut() {
            Some((count, duration)) if *duration == s.duration => *count += 1,
            _ => runs.push((1, s.duration)),
        }
    }
    w.begin_full(b"stts", 0, 0);
    w.u32(runs.len() as u32);
    for (count, duration) in runs {
        w.u32(count);
        w.u32(duration);
    }
    w.end();

    if with_sync {
        let sync: Vec<u32> = samples.iter().enumerate().filter(|(_, s)| s.sync).map(|(i, _)| i as u32 + 1).collect();
        w.begin_full(b"stss", 0, 0);
        w.u32(sync.len() as u32);
        for index in sync {
            w.u32(index);
        }
        w.end();
    }

    w.begin_full(b"stsz", 0, 0);
    w.u32(0);
    w.u32(samples.len() as u32);
    for s in samples {
        w.u32(s.size);
    }
    w.end();

    // Chunk = amostras contíguas no arquivo (uma sequência por fragmento).
    let mut chunks: Vec<(u64, u32)> = Vec::new();
    let mut previous_end = None;
    for s in samples {
        match chunks.last_mut() {
            Some((_, count)) if previous_end == Some(s.offset) => *count += 1,
            _ => chunks.push((s.offset, 1)),
        }
        previous_end = Some(s.offset + s.size as u64);
    }
    let mut stsc: Vec<(u32, u32)> = Vec::new();
    for (i, (_, count)) in chunks.iter().enumerate() {
        if stsc.last().map(|(_, c)| c) != Some(count) {
            stsc.push((i as u32 + 1, *count));
        }
    }
    w.begin_full(b"stsc", 0, 0);
    w.u32(stsc.len() as u32);
    for (first, count) in stsc {
        w.u32(first);
        w.u32(count);
        w.u32(1);
    }
    w.end();

    let large = chunks.last().is_some_and(|(offset, _)| *offset > u32::MAX as u64);
    w.begin_full(if large { b"co64" } else { b"stco" }, 0, 0);
    w.u32(chunks.len() as u32);
    for (offset, _) in chunks {
        if large {
            w.u64(offset);
        } else {
            w.u32(offset as u32);
        }
    }
    w.end();
}

/// `index`: amostras do vídeo e de cada faixa de áudio (MP4 final); `None` = fragmentado.
fn moov(video: &VideoTrack, audio: &[AudioTrack], index: Option<(&[IndexedSample], &[Vec<IndexedSample>])>) -> Vec<u8> {
    let total = |samples: &[IndexedSample]| samples.iter().map(|s| s.duration as u64).sum::<u64>();
    let video_duration = index.map_or(0, |(v, _)| total(v));
    let audio_durations: Vec<u64> = (0..audio.len()).map(|i| index.and_then(|(_, a)| a.get(i)).map_or(0, |a| total(a))).collect();
    let video_ms = video_duration * 1000 / VIDEO_TIMESCALE as u64;
    let audio_ms: Vec<u64> = audio.iter().zip(&audio_durations).map(|(a, d)| d * 1000 / a.sample_rate as u64).collect();

    let mut w = Writer::default();
    w.begin(b"moov");
    mvhd(&mut w, audio_ms.iter().copied().fold(video_ms, u64::max), AUDIO_TRACK + audio.len() as u32);

    w.begin(b"trak");
    tkhd(&mut w, VIDEO_TRACK, video_ms, false, true, 0, video.width, video.height);
    w.begin(b"mdia");
    mdhd(&mut w, VIDEO_TIMESCALE, video_duration);
    hdlr(&mut w, b"vide", "CapturaX Video");
    w.begin(b"minf");
    w.begin_full(b"vmhd", 0, 1);
    w.zeros(8);
    w.end();
    dinf(&mut w);
    w.begin(b"stbl");
    stsd(&mut w, Some(video), None);
    match index {
        Some((samples, _)) => sample_tables(&mut w, samples, true),
        None => empty_tables(&mut w),
    }
    w.end();
    w.end();
    w.end();
    w.end();

    let group = if audio.len() > 1 { 1 } else { 0 };
    for (i, track) in audio.iter().enumerate() {
        w.begin(b"trak");
        tkhd(&mut w, AUDIO_TRACK + i as u32, audio_ms[i], true, i == 0, group, 0, 0);
        w.begin(b"mdia");
        mdhd(&mut w, track.sample_rate, audio_durations[i]);
        hdlr(&mut w, b"soun", &track.name);
        w.begin(b"minf");
        w.begin_full(b"smhd", 0, 0);
        w.u32(0);
        w.end();
        dinf(&mut w);
        w.begin(b"stbl");
        stsd(&mut w, None, Some(track));
        match index {
            Some((_, samples)) => sample_tables(&mut w, samples.get(i).map_or(&[], |s| s.as_slice()), false),
            None => empty_tables(&mut w),
        }
        w.end();
        w.end();
        w.end();
        w.end();
    }

    if index.is_none() {
        w.begin(b"mvex");
        for track in VIDEO_TRACK..AUDIO_TRACK + audio.len() as u32 {
            w.begin_full(b"trex", 0, 0);
            w.u32(track);
            w.u32(1);
            w.u32(0);
            w.u32(0);
            w.u32(0);
            w.end();
        }
        w.end();
    }
    w.end();
    w.buf
}

/// Segmento de inicialização (`ftyp` + `moov` vazio com `mvex`). Devolve também onde o `moov`
/// começa, para o arquivo final poder apagá-lo depois.
pub fn init_segment(video: &VideoTrack, audio: &[AudioTrack]) -> (Vec<u8>, usize) {
    let mut w = Writer::default();
    ftyp(&mut w);
    let moov_offset = w.buf.len();
    w.bytes(&moov(video, audio, None));
    (w.buf, moov_offset)
}

/// `moov` completo (não fragmentado) para o fim do arquivo gravado.
pub fn progressive_moov(video: &VideoTrack, audio: &[AudioTrack], video_samples: &[IndexedSample], audio_samples: &[Vec<IndexedSample>]) -> Vec<u8> {
    moov(video, audio, Some((video_samples, audio_samples)))
}

pub struct Sample {
    pub data: Vec<u8>,
    /// Instante de decodificação no timescale da faixa.
    pub dts: u64,
    pub sync: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct SampleMeta {
    pub size: u32,
    pub duration: u32,
    pub sync: bool,
}

pub struct Fragment {
    pub bytes: Vec<u8>,
    /// Início das amostras dentro de `bytes` (logo depois do cabeçalho do `mdat`).
    pub payload_offset: usize,
    pub video: Vec<SampleMeta>,
    /// Uma lista por faixa de áudio.
    pub audio: Vec<Vec<SampleMeta>>,
    pub duration_secs: f64,
    /// Começa num keyframe (dá para começar a tocar daqui).
    pub starts_with_key: bool,
}

/// Junta amostras e fecha um fragmento a cada keyframe (respeitando uma duração mínima) ou,
/// no modo de baixa latência, também a cada `part` (fragmentos que não começam em keyframe).
pub struct Fragmenter {
    sequence: u32,
    min_duration: u64,
    part: Option<u64>,
    audio_rate: u32,
    video: Vec<Sample>,
    /// Uma fila por faixa de áudio.
    audio: Vec<Vec<Sample>>,
    last_video_duration: u32,
    /// Já recebeu o primeiro keyframe (antes dele, o vídeo é descartado).
    started: bool,
}

impl Fragmenter {
    pub fn new(min_duration_secs: f64, part_secs: Option<f64>, audio_rate: u32, audio_tracks: usize) -> Self {
        let ticks = |secs: f64| (secs * VIDEO_TIMESCALE as f64) as u64;
        Self {
            sequence: 0,
            min_duration: ticks(min_duration_secs),
            part: part_secs.map(ticks),
            audio_rate,
            video: Vec::new(),
            audio: (0..audio_tracks).map(|_| Vec::new()).collect(),
            last_video_duration: VIDEO_TIMESCALE / 30,
            started: false,
        }
    }

    pub fn push_audio(&mut self, track: usize, sample: Sample) {
        self.audio[track].push(sample);
    }

    /// Fecha o fragmento anterior quando chega um keyframe e já passou a duração mínima.
    pub fn push_video(&mut self, sample: Sample) -> Option<Fragment> {
        let elapsed = self.video.first().map(|first| sample.dts.saturating_sub(first.dts));
        let ready = match (elapsed, self.part) {
            (None, _) => false,
            (Some(elapsed), Some(part)) => sample.sync || elapsed >= part,
            (Some(elapsed), None) => sample.sync && elapsed >= self.min_duration,
        };
        let fragment = if ready { self.cut(Some(sample.dts)) } else { None };
        // O vídeo precisa começar num keyframe; antes do primeiro, descarta. Depois, pedaços de
        // baixa latência podem começar no meio do GOP normalmente.
        self.started |= sample.sync;
        if self.started {
            self.video.push(sample);
        }
        fragment
    }

    /// Fecha o que sobrou (fim da gravação/transmissão).
    pub fn finish(&mut self) -> Option<Fragment> {
        self.cut(None)
    }

    fn cut(&mut self, next_video_dts: Option<u64>) -> Option<Fragment> {
        if self.video.is_empty() {
            return None;
        }
        let video = std::mem::take(&mut self.video);
        let mut durations: Vec<u32> = video.windows(2).map(|pair| (pair[1].dts - pair[0].dts) as u32).collect();
        let last = match next_video_dts {
            Some(next) => (next - video.last().unwrap().dts) as u32,
            None => self.last_video_duration,
        };
        durations.push(last.max(1));
        self.last_video_duration = *durations.last().unwrap();

        // Áudio até o instante em que o próximo fragmento começa.
        let end_audio = next_video_dts.map(|end| end * self.audio_rate as u64 / VIDEO_TIMESCALE as u64);
        let audio: Vec<Vec<Sample>> = self
            .audio
            .iter_mut()
            .map(|queue| match end_audio {
                Some(end) => {
                    let split = queue.iter().position(|s| s.dts >= end).unwrap_or(queue.len());
                    queue.drain(..split).collect()
                }
                None => std::mem::take(queue),
            })
            .collect();

        self.sequence += 1;
        let duration_secs = durations.iter().map(|&d| d as f64).sum::<f64>() / VIDEO_TIMESCALE as f64;
        let video_meta: Vec<SampleMeta> = video
            .iter()
            .zip(&durations)
            .map(|(s, &duration)| SampleMeta { size: s.data.len() as u32, duration, sync: s.sync })
            .collect();
        let audio_meta: Vec<Vec<SampleMeta>> = audio
            .iter()
            .map(|track| track.iter().map(|s| SampleMeta { size: s.data.len() as u32, duration: crate::media::aac::FRAME_SAMPLES, sync: true }).collect())
            .collect();

        let mut w = Writer::default();
        w.begin(b"moof");
        w.begin_full(b"mfhd", 0, 0);
        w.u32(self.sequence);
        w.end();

        // Ordem no mdat: vídeo e depois cada faixa de áudio.
        let mut offset_fields = Vec::new();
        let mut payload = 0usize;
        let tracks = std::iter::once((VIDEO_TRACK, &video, &video_meta)).chain(audio.iter().zip(&audio_meta).enumerate().map(|(i, (s, m))| (AUDIO_TRACK + i as u32, s, m)));
        for (track, samples, meta) in tracks {
            if samples.is_empty() {
                continue;
            }
            let bytes: usize = samples.iter().map(|s| s.data.len()).sum();
            let is_video = track == VIDEO_TRACK;
            w.begin(b"traf");
            w.begin_full(b"tfhd", 0, 0x02_0000); // default-base-is-moof
            w.u32(track);
            w.end();
            w.begin_full(b"tfdt", 1, 0);
            w.u64(samples[0].dts);
            w.end();
            let flags = 0x1 | 0x100 | 0x200 | if is_video { 0x400 } else { 0 };
            w.begin_full(b"trun", 0, flags);
            w.u32(samples.len() as u32);
            offset_fields.push((w.buf.len(), payload));
            payload += bytes;
            w.u32(0); // data_offset, preenchido abaixo
            for m in meta.iter() {
                w.u32(m.duration);
                w.u32(m.size);
                if is_video {
                    w.u32(if m.sync { 0x0200_0000 } else { 0x0101_0000 });
                }
            }
            w.end();
            w.end();
        }
        w.end();

        let moof_size = w.buf.len();
        for (position, start) in offset_fields {
            let offset = moof_size + 8 + start;
            w.buf[position..position + 4].copy_from_slice(&(offset as u32).to_be_bytes());
        }

        w.u32((8 + payload) as u32);
        w.bytes(b"mdat");
        for s in video.iter().chain(audio.iter().flatten()) {
            w.bytes(&s.data);
        }

        let starts_with_key = video[0].sync;
        Some(Fragment { bytes: w.buf, payload_offset: moof_size + 8, video: video_meta, audio: audio_meta, duration_secs, starts_with_key })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(dts_frames: u64, sync: bool) -> Sample {
        Sample { data: vec![0, 0, 0, 1, 0x65], dts: dts_frames * 1500, sync }
    }

    /// Modo baixa latência: pedaços de 100 ms no meio do GOP não podem perder frames.
    #[test]
    fn low_latency_parts_keep_every_frame() {
        let mut fragmenter = Fragmenter::new(1.0, Some(0.1), 48_000, 1);
        let mut frames_out = 0;
        let mut parts = 0;
        for i in 0..240u64 {
            // 60 fps, keyframe a cada 60 frames; começa com 3 frames antes do primeiro keyframe.
            let sync = i >= 3 && (i - 3) % 60 == 0;
            if let Some(fragment) = fragmenter.push_video(video(i, sync)) {
                frames_out += fragment.video.len();
                parts += 1;
            }
        }
        frames_out += fragmenter.finish().map_or(0, |f| f.video.len());
        assert_eq!(frames_out, 237, "only the 3 frames before the first keyframe may be dropped");
        assert!(parts >= 30, "expected ~100 ms parts, got {parts}");
    }
}
