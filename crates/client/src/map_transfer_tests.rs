use super::MapDownload;
use sendit_sim::{
    wire::{self, ServerMsg, CHUNK_BYTES},
    Arena,
};
use std::time::{Duration, Instant};

const TOKEN: u64 = 0x372c_51f4_11ee_9f20;
const MAP: &str = "community-drill";

fn fixture() -> (Arena, Vec<Vec<u8>>) {
    let mut arena = Arena::load("drill").unwrap();
    arena.name = MAP.into();
    let bytes = serde_json::to_vec(&arena).unwrap();
    let chunks: Vec<_> = bytes.chunks(CHUNK_BYTES).map(<[u8]>::to_vec).collect();
    assert!(chunks.len() >= 4);
    (arena, chunks)
}

fn deliver(
    download: &mut MapDownload,
    map: &str,
    token: u64,
    part: u8,
    count: u8,
    data: Vec<u8>,
) -> Result<Option<Arena>, String> {
    let encoded = wire::encode(&ServerMsg::MapChunk {
        token,
        map: map.into(),
        part,
        count,
        data,
    })?;
    let ServerMsg::MapChunk {
        token,
        map,
        part,
        count,
        data,
    } = wire::decode(&encoded)?
    else {
        unreachable!()
    };
    download.accept(token, &map, part, count, data)
}

#[test]
fn lost_manifest_and_middle_chunk_recover_after_reordering_duplicates_and_stale_packets() {
    let (expected, chunks) = fixture();
    let count = chunks.len() as u8;
    let mut download = MapDownload::new(MAP, TOKEN);
    let start = Instant::now();
    assert_eq!(download.requests(start), vec![0]);
    // Lose the first reply. A datagram from an old session or map cannot unblock
    // the missing manifest or allocate slots for the wrong transfer.
    assert!(
        deliver(&mut download, MAP, TOKEN - 1, 0, count, chunks[0].clone())
            .unwrap()
            .is_none()
    );
    assert!(deliver(
        &mut download,
        "other-map",
        TOKEN,
        0,
        count,
        chunks[0].clone()
    )
    .unwrap()
    .is_none());
    assert!(download
        .requests(start + Duration::from_millis(80))
        .is_empty());
    let retry = start + Duration::from_millis(200);
    assert_eq!(download.requests(retry), vec![0]);
    assert!(
        deliver(&mut download, MAP, TOKEN, 0, count, chunks[0].clone())
            .unwrap()
            .is_none()
    );

    let mut lost_middle = false;
    let mut retried_middle = false;
    let mut duplicated = false;
    let mut completed = None;
    for round in 0..12 {
        let now = retry + Duration::from_millis(220 * round + 1);
        let batch = download.requests(now);
        assert!(batch.len() <= 8);
        // Later parts arrive first. Drop one request exactly once, and resend a
        // valid part before the missing part's timer fires.
        for part in batch.into_iter().rev() {
            assert!(part < count);
            if part == 2 && !lost_middle {
                lost_middle = true;
                continue;
            }
            if part == 2 && lost_middle {
                retried_middle = true;
            }
            let data = chunks[part as usize].clone();
            let complete = deliver(&mut download, MAP, TOKEN, part, count, data.clone()).unwrap();
            if let Some(arena) = complete {
                completed = Some(arena);
                break;
            }
            if part > 2 && !duplicated {
                assert!(deliver(&mut download, MAP, TOKEN, part, count, data)
                    .unwrap()
                    .is_none());
                duplicated = true;
            }
        }
        if completed.is_some() {
            break;
        }
    }
    assert!(lost_middle && retried_middle && duplicated);
    assert_eq!(
        serde_json::to_value(completed.expect("a validated visible map")).unwrap(),
        serde_json::to_value(expected).unwrap(),
    );
}

#[test]
fn authenticated_invalid_part_count_length_and_map_identity_are_rejected() {
    for (part, count, len) in [
        (0, 0, 1),
        (2, 2, 1),
        (0, 255, CHUNK_BYTES),
        (0, 2, 3),
        (0, 1, CHUNK_BYTES + 1),
    ] {
        let mut download = MapDownload::new(MAP, TOKEN);
        assert!(
            deliver(&mut download, MAP, TOKEN, part, count, vec![b' '; len]).is_err(),
            "part {part}, count {count}, bytes {len}"
        );
    }
    let (_, chunks) = fixture();
    let count = chunks.len() as u8;
    let mut download = MapDownload::new(MAP, TOKEN);
    assert!(
        deliver(&mut download, MAP, TOKEN, 0, count, chunks[0].clone())
            .unwrap()
            .is_none()
    );
    assert!(deliver(&mut download, MAP, TOKEN, 1, count + 1, chunks[1].clone()).is_err());

    let mut wrong = MapDownload::new("different", TOKEN);
    for (index, chunk) in chunks.into_iter().enumerate() {
        let out = deliver(&mut wrong, "different", TOKEN, index as u8, count, chunk);
        if index + 1 == count as usize {
            assert!(
                out.is_err(),
                "wire label must agree with the map's validated name"
            );
        } else {
            assert!(out.unwrap().is_none());
        }
    }
}
