// Copyright 2021 Datafuse Labs
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::io::Write;

use super::write;
use super::NativeWriter;
use crate::arrow::array::*;
use crate::arrow::chunk::Chunk;
use crate::arrow::error::Result;
use crate::native::compression::CommonCompression;
use crate::native::compression::Compression;
use crate::native::nested::slice_nest_array;
use crate::native::nested::to_leaves;
use crate::native::nested::to_nested;
use crate::native::ColumnMeta;
use crate::native::PageMeta;
use crate::native::EOF_MARKER;

/// Options declaring the behaviour of writing to IPC
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WriteOptions {
    /// Whether the buffers should be compressed and which codec to use.
    /// Note: to use compression the crate must be compiled with feature `io_ipc_compression`.
    pub default_compression: CommonCompression,
    /// If some encoding method performs over this ratio, we will switch to use it.
    pub default_compress_ratio: Option<f64>,
    pub max_page_size: Option<usize>,
    pub forbidden_compressions: Vec<Compression>,
}

impl<W: Write> NativeWriter<W> {
    /// Encode and write a [`Chunk`] to the file
    pub fn encode_chunk(&mut self, chunk: &Chunk<Box<dyn Array>>) -> Result<()> {
        let page_size = self
            .options
            .max_page_size
            .unwrap_or(chunk.len())
            .min(chunk.len());

        for (array, field) in chunk.arrays().iter().zip(self.schema.fields.iter()) {
            let length = array.len();

            let nested = to_nested(array.as_ref(), field)?;
            let leaf_arrays = to_leaves(array.as_ref());

            for (leaf_array, nested) in leaf_arrays.iter().zip(nested.into_iter()) {
                let leaf_array = leaf_array.to_boxed();
                let mut page_metas = Vec::with_capacity((length + 1) / page_size + 1);
                let start = self.writer.offset;

                for offset in (0..length).step_by(page_size) {
                    let length = if offset + page_size > length {
                        length - offset
                    } else {
                        page_size
                    };

                    let mut sub_array = leaf_array.clone();
                    let mut sub_nested = nested.clone();
                    slice_nest_array(sub_array.as_mut(), &mut sub_nested, offset, length);

                    {
                        let page_start = self.writer.offset;
                        write(
                            &mut self.writer,
                            sub_array.as_ref(),
                            &sub_nested,
                            self.options.clone(),
                            &mut self.scratch,
                        )
                        .unwrap();

                        let page_end = self.writer.offset;
                        page_metas.push(PageMeta {
                            length: (page_end - page_start),
                            num_values: sub_array.len() as u64,
                        });
                    }
                }

                self.metas.push(ColumnMeta {
                    offset: start,
                    pages: page_metas,
                })
            }
        }
        Ok(())
    }
}

/// Write a record batch to the writer, writing the message size before the message
/// if the record batch is being written to a stream
pub fn write_eof<W: Write>(writer: &mut W, total_len: i32) -> Result<usize> {
    writer.write_all(&EOF_MARKER)?;
    writer.write_all(&total_len.to_le_bytes()[..])?;
    Ok(8)
}
