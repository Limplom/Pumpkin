use pumpkin_data::block_properties::{BlockProperties, OakLeavesLikeProperties};
use pumpkin_data::tag::{Block as BlockTag, Taggable};
use pumpkin_data::{Block, BlockDirection, BlockStateId};
use pumpkin_macros::pumpkin_block_from_tag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockFlags;

use crate::block::{
    BlockBehaviour, BlockFuture, GetStateForNeighborUpdateArgs, OnPlaceArgs, OnScheduledTickArgs,
};
use crate::world::World;

type LeavesProperties = OakLeavesLikeProperties;

/// The maximum (and default) distance a leaf can be from a log before it decays.
const MAX_DISTANCE: u8 = 7;

/// All leaf blocks (`minecraft:leaves`) share the same `distance` / `persistent`
/// / `waterlogged` state, so a single behaviour handles the whole tag.
#[pumpkin_block_from_tag("minecraft:leaves")]
pub struct LeavesBlock;

impl BlockBehaviour for LeavesBlock {
    /// `getPlacementState` in vanilla: player-placed leaves are `persistent`,
    /// but the `distance` is still computed from the surrounding blocks.
    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let mut props = LeavesProperties::default(args.block);
            props.persistent = true;
            props.waterlogged = args.replacing.water_source();
            props.distance = calculate_distance(args.world, args.position);
            props.to_state_id(args.block)
        })
    }

    /// `getStateForNeighborUpdate` in vanilla: schedule a tick so the distance is
    /// recomputed. The state itself is returned unchanged.
    fn get_state_for_neighbor_update<'a>(
        &'a self,
        args: GetStateForNeighborUpdateArgs<'a>,
    ) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let props = LeavesProperties::from_state_id(args.state_id, args.block);
            let neighbor_block = Block::from_state_id(args.neighbor_state_id);
            let neighbor_distance =
                get_distance_from_log(neighbor_block, args.neighbor_state_id).saturating_add(1);
            // Only schedule a recompute if this update could actually change the
            // distance (mirrors vanilla's early-out).
            if neighbor_distance != 1 || props.distance != neighbor_distance {
                args.world
                    .schedule_block_tick(args.block, *args.position, 1, TickPriority::Normal);
            }
            args.state_id
        })
    }

    /// `scheduledTick` in vanilla: recompute the distance and either decay the
    /// leaf (non-persistent and out of range) or update the `distance` state.
    fn on_scheduled_tick<'a>(&'a self, args: OnScheduledTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let world = args.world.as_ref();
            let state_id = world.get_block_state_id(args.position);
            let block = Block::from_state_id(state_id);
            // The block here may have changed between scheduling and this tick;
            // only recompute if it is still a leaf so we never write a leaf state
            // over a different block.
            if !block.has_tag(&BlockTag::MINECRAFT_LEAVES) {
                return;
            }
            let mut props = LeavesProperties::from_state_id(state_id, block);
            let distance = calculate_distance(world, args.position);

            if distance == MAX_DISTANCE && !props.persistent {
                // Decay: break the leaf and drop its items.
                args.world
                    .break_block(args.position, None, BlockFlags::empty())
                    .await;
            } else if props.distance != distance {
                props.distance = distance;
                args.world
                    .set_block_state(
                        args.position,
                        props.to_state_id(block),
                        BlockFlags::NOTIFY_ALL,
                    )
                    .await;
            }
        })
    }
}

/// Vanilla `updateDistanceFromLogs`: the leaf's distance is `1 +` the smallest
/// neighbor contribution, capped at [`MAX_DISTANCE`].
fn calculate_distance(world: &World, pos: &BlockPos) -> u8 {
    combine_neighbor_distances(BlockDirection::all().into_iter().map(|direction| {
        let neighbor_pos = pos.offset(direction.to_offset());
        let (block, state_id) = world.get_block_and_state_id(&neighbor_pos);
        get_distance_from_log(block, state_id)
    }))
}

/// Vanilla `getDistanceFromLog`: a log contributes `0`, another leaf contributes
/// its own `distance`, and anything else contributes [`MAX_DISTANCE`].
fn get_distance_from_log(block: &Block, state_id: BlockStateId) -> u8 {
    if block.has_tag(&BlockTag::MINECRAFT_LOGS) {
        0
    } else if block.has_tag(&BlockTag::MINECRAFT_LEAVES) {
        LeavesProperties::from_state_id(state_id, block).distance
    } else {
        MAX_DISTANCE
    }
}

/// Folds the six neighbor contributions into a final distance: `1 +` the minimum
/// contribution, capped at [`MAX_DISTANCE`]. Short-circuits once the minimum
/// possible value (`1`) is reached.
fn combine_neighbor_distances(contributions: impl IntoIterator<Item = u8>) -> u8 {
    let mut distance = MAX_DISTANCE;
    for contribution in contributions {
        distance = distance.min(contribution.saturating_add(1));
        if distance == 1 {
            break;
        }
    }
    distance
}

#[cfg(test)]
mod tests {
    use super::{MAX_DISTANCE, combine_neighbor_distances, get_distance_from_log};
    use pumpkin_data::Block;
    use pumpkin_data::BlockStateId;
    use pumpkin_data::block_properties::{BlockProperties, OakLeavesLikeProperties};

    fn leaf_state_with_distance(distance: u8) -> BlockStateId {
        let mut props = OakLeavesLikeProperties::default(&Block::OAK_LEAVES);
        props.distance = distance;
        props.to_state_id(&Block::OAK_LEAVES)
    }

    #[test]
    fn log_contributes_zero() {
        assert_eq!(
            get_distance_from_log(&Block::OAK_LOG, Block::OAK_LOG.default_state.id),
            0
        );
    }

    #[test]
    fn non_log_non_leaf_contributes_max() {
        assert_eq!(
            get_distance_from_log(&Block::STONE, Block::STONE.default_state.id),
            MAX_DISTANCE
        );
    }

    #[test]
    fn leaf_contributes_its_own_distance() {
        let state_id = leaf_state_with_distance(3);
        assert_eq!(get_distance_from_log(&Block::OAK_LEAVES, state_id), 3);
    }

    #[test]
    fn adjacent_to_log_is_distance_one() {
        // One neighbor is a log (contribution 0), the rest are unrelated (7).
        assert_eq!(
            combine_neighbor_distances([0, MAX_DISTANCE, MAX_DISTANCE, MAX_DISTANCE]),
            1
        );
    }

    #[test]
    fn chained_leaves_increase_distance() {
        // Nearest leaf neighbor has distance 1, so this leaf is 2.
        assert_eq!(
            combine_neighbor_distances([1, 4, MAX_DISTANCE, MAX_DISTANCE]),
            2
        );
    }

    #[test]
    fn isolated_leaf_caps_at_max() {
        // No log or leaf nearby: every neighbor contributes MAX_DISTANCE.
        assert_eq!(combine_neighbor_distances([MAX_DISTANCE; 6]), MAX_DISTANCE);
    }
}
