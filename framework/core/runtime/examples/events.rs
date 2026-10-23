//! CPU-free event demonstration, not a firmware or whole-machine emulator.

use std::collections::BTreeSet;

use mosaic_contract_model::Ctx;
use mosaic_core_runtime::{EventKind, EventQueue, ModelContext};
use mosaic_core_types::{NodeId, SignalId, Snapshot, VTime};

fn trace(queue: &mut EventQueue) -> Result<String, String> {
    let signals = BTreeSet::from([7]);
    let mut lines = Vec::new();
    while let Some(event) = queue.pop_next() {
        match event.kind {
            EventKind::Timer { token } => {
                lines.push(format!(
                    "time={} node={} timer={token}",
                    event.at.0, event.owner.0
                ));
                let mut ctx = ModelContext::new(queue, event.owner, &signals);
                match token {
                    100 => {
                        ctx.schedule_after(VTime(10), 101)
                            .map_err(|e| format!("{e:?}"))?;
                    }
                    200 => ctx
                        .set_signal(SignalId(7), false)
                        .map_err(|e| format!("{e:?}"))?,
                    _ => {}
                }
            }
            EventKind::Signal { signal, high } => lines.push(format!(
                "time={} node={} signal={} high={high}",
                event.at.0, event.owner.0, signal.0
            )),
            _ => return Err("unsupported event kind".into()),
        }
    }
    Ok(lines.join("\n") + "\n")
}

fn main() -> Result<(), String> {
    let signals = BTreeSet::from([7]);
    let mut queue = EventQueue::new(16);
    {
        let mut ctx = ModelContext::new(&mut queue, NodeId(1), &signals);
        ctx.set_signal(SignalId(7), true)
            .map_err(|e| format!("{e:?}"))?;
        ctx.schedule(VTime(10), 100).map_err(|e| format!("{e:?}"))?;
    }
    ModelContext::new(&mut queue, NodeId(2), &signals)
        .schedule(VTime(10), 200)
        .map_err(|e| format!("{e:?}"))?;

    let checkpoint = queue.save();
    let expected = trace(&mut queue)?;
    let mut restored = EventQueue::new(16);
    restored
        .load(EventQueue::SCHEMA_VERSION, &checkpoint)
        .map_err(|e| format!("{e:?}"))?;
    let replayed = trace(&mut restored)?;
    if expected != replayed || queue.save() != restored.save() {
        return Err("snapshot replay diverged".into());
    }
    println!("{expected}snapshot replay: identical");
    Ok(())
}
