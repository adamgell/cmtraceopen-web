//! Synthetic bundle generator. Samples per-bundle file counts, parser-kind
//! mix, and per-file sizes from a `Shape`, then writes lines templated per
//! parser kind so the api-server's parser does meaningful work.

use crate::bundle::Bundle;
use crate::corpus::shape::Shape;
use crate::corpus::Corpus;
use rand::distr::weighted::WeightedIndex;
use rand::prelude::*;
use rand_chacha::ChaCha20Rng;
use std::io::Write;

const BUNDLE_SIZE_CAP: u64 = 49 * 1024 * 1024; // stay under the 50 MiB API cap

pub struct SyntheticCorpus {
    shape: Shape,
    rng: ChaCha20Rng,
    parser_kind_keys: Vec<String>,
    parser_kind_dist: WeightedIndex<f64>,
    encoding_keys: Vec<String>,
    encoding_dist: WeightedIndex<f64>,
}

impl SyntheticCorpus {
    pub fn new(shape: Shape, seed: u64) -> anyhow::Result<Self> {
        let parser_kind_keys: Vec<String> = shape.parser_kind_weights.keys().cloned().collect();
        let parser_kind_weights: Vec<f64> = parser_kind_keys
            .iter()
            .map(|k| shape.parser_kind_weights[k])
            .collect();
        let parser_kind_dist = WeightedIndex::new(&parser_kind_weights)?;

        let encoding_keys: Vec<String> = shape.encoding_weights.keys().cloned().collect();
        let encoding_weights: Vec<f64> = encoding_keys
            .iter()
            .map(|k| shape.encoding_weights[k])
            .collect();
        let encoding_dist = WeightedIndex::new(&encoding_weights)?;

        Ok(Self {
            shape,
            rng: ChaCha20Rng::seed_from_u64(seed),
            parser_kind_keys,
            parser_kind_dist,
            encoding_keys,
            encoding_dist,
        })
    }

    fn render_line(kind: &str, i: usize) -> String {
        match kind {
            "Ccm" => format!(
                "<![LOG[Synthetic CCM line {i}]LOG]!><time=\"12:34:56.000+000\" date=\"04-29-2026\" component=\"SyntheticComp\" context=\"\" type=\"1\" thread=\"100\" file=\"synth.cpp:42\">"
            ),
            "IisW3c" => format!(
                "2026-04-29 12:34:{i:02} W3SVC1 GET /path - 80 - 10.0.0.{i} Mozilla/5.0 200 0 0 42"
            ),
            "Timestamped" => format!("[2026-04-29T12:34:{i:02}Z] INFO synthesizer: line {i}"),
            "TracingJson" => format!("{{\"timestamp\":\"2026-04-29T12:34:{i:02}Z\",\"level\":\"INFO\",\"target\":\"synth\",\"message\":\"line {i}\"}}"),
            "Setup" => format!("MIF: Action 'SyntheticAction' returned 0 ({i})"),
            _ => format!("synthetic plain line {i}\n"),
        }
    }

    fn build_zip(&mut self, n_files: u32) -> anyhow::Result<(Vec<u8>, u32)> {
        let mut buf = Vec::new();
        {
            let mut zw = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            let mut size_so_far: u64 = 0;
            let mut emitted: u32 = 0;
            for i in 0..n_files {
                let kind_idx = self.parser_kind_dist.sample(&mut self.rng);
                let kind = self.parser_kind_keys[kind_idx].clone();
                let target_bytes = self.shape.bytes_per_file.sample(self.rng.random::<f64>());
                let target_bytes = target_bytes.min(BUNDLE_SIZE_CAP - size_so_far);
                if target_bytes < 32 {
                    break;
                }
                let mut body = String::with_capacity(target_bytes as usize);
                let mut line_i = 0usize;
                while (body.len() as u64) < target_bytes {
                    body.push_str(&Self::render_line(&kind, line_i));
                    body.push('\n');
                    line_i += 1;
                }
                let path = format!("logs/{kind}/synth-{i:03}.log");
                zw.start_file(&path, opts)?;
                zw.write_all(body.as_bytes())?;
                size_so_far = size_so_far.saturating_add(body.len() as u64);
                emitted += 1;
                if size_so_far >= BUNDLE_SIZE_CAP {
                    break;
                }
            }
            zw.finish()?;
            // Keep RNG stream stable for future encoding variants. The drawn value
            // is currently unused (UTF-16LE/Other emission is future work); see
            // spec doc § "encoding variation" hooks.
            let _ = self.encoding_dist.sample(&mut self.rng);
            let _ = &self.encoding_keys;
            let _ = emitted;
        }
        let n_files_actually = {
            let cursor = std::io::Cursor::new(&buf);
            zip::ZipArchive::new(cursor)?.len() as u32
        };
        Ok((buf, n_files_actually))
    }
}

#[async_trait::async_trait]
impl Corpus for SyntheticCorpus {
    async fn next(&mut self) -> anyhow::Result<Bundle> {
        let n_files = self.shape.files_per_bundle.sample(self.rng.random::<f64>()) as u32;
        let n_files = n_files.max(1);
        let (zip_bytes, n_files_actual) = self.build_zip(n_files)?;
        Ok(Bundle {
            seq: 0,                       // driver fills this in
            device_id: String::new(),     // driver fills this in
            bundle_id: uuid::Uuid::now_v7(),
            n_files: n_files_actual,
            zip_bytes,
        })
    }
}
