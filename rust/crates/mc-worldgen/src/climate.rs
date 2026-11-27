use mc_data::overworld_biomes::{OVERWORLD_BIOME_NAMES, OVERWORLD_CLIMATE_ENTRIES};
use std::cmp::Ordering;

const DIMS: usize = 7;
const CHILDREN_PER_NODE: usize = 19; // Exact Minecraft 26.3 Climate.RTree.CHILDREN_PER_NODE

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClimateParameter {
    pub min: i64,
    pub max: i64,
}

impl ClimateParameter {
    #[inline]
    pub const fn new(min: i64, max: i64) -> Self {
        Self { min, max }
    }

    #[inline]
    pub const fn distance(self, target: i64) -> i64 {
        let above = target - self.max;
        let below = self.min - target;
        if above > 0 {
            above
        } else if below > 0 {
            below
        } else {
            0
        }
    }

    #[inline]
    pub const fn span(self, other: Self) -> Self {
        Self {
            min: if self.min < other.min { self.min } else { other.min },
            max: if self.max > other.max { self.max } else { other.max },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetPoint {
    pub temperature: i64,
    pub humidity: i64,
    pub continentalness: i64,
    pub erosion: i64,
    pub depth: i64,
    pub weirdness: i64,
}

impl TargetPoint {
    #[inline]
    pub fn from_floats(
        temperature: f32,
        humidity: f32,
        continentalness: f32,
        erosion: f32,
        depth: f32,
        weirdness: f32,
    ) -> Self {
        Self {
            temperature: quantize_coord(temperature),
            humidity: quantize_coord(humidity),
            continentalness: quantize_coord(continentalness),
            erosion: quantize_coord(erosion),
            depth: quantize_coord(depth),
            weirdness: quantize_coord(weirdness),
        }
    }

    #[inline]
    pub fn to_array(self) -> [i64; DIMS] {
        [
            self.temperature,
            self.humidity,
            self.continentalness,
            self.erosion,
            self.depth,
            self.weirdness,
            0,
        ]
    }
}

#[inline]
pub fn quantize_coord(coord: f32) -> i64 {
    (coord * 10000.0_f32) as i64
}

#[derive(Debug, Clone)]
enum RNode<T> {
    Leaf {
        space: [ClimateParameter; DIMS],
        value: T,
    },
    SubTree {
        space: [ClimateParameter; DIMS],
        children: Vec<RNode<T>>,
    },
}

impl<T> RNode<T> {
    #[inline]
    fn space(&self) -> &[ClimateParameter; DIMS] {
        match self {
            Self::Leaf { space, .. } | Self::SubTree { space, .. } => space,
        }
    }

    #[inline]
    fn distance(&self, target: &[i64; DIMS]) -> i64 {
        let sp = self.space();
        let mut dist = 0i64;
        for i in 0..DIMS {
            let d = sp[i].distance(target[i]);
            dist += d * d;
        }
        dist
    }

    fn from_children(children: Vec<Self>) -> Self {
        let mut bounds = *children[0].space();
        for c in &children[1..] {
            let cs = c.space();
            for d in 0..DIMS {
                bounds[d] = bounds[d].span(cs[d]);
            }
        }
        Self::SubTree {
            space: bounds,
            children,
        }
    }
}

/// Exact port of Minecraft 26.3's 7D `Climate.RTree<T>` with 19-way branching.
#[derive(Debug, Clone)]
pub struct ClimateRTree<T> {
    root: RNode<T>,
}

impl<T: Copy> ClimateRTree<T> {
    pub fn create(entries: Vec<([ClimateParameter; DIMS], T)>) -> Self {
        assert!(!entries.is_empty(), "RTree requires at least 1 entry");
        let leaves: Vec<RNode<T>> = entries
            .into_iter()
            .map(|(space, value)| RNode::Leaf { space, value })
            .collect();
        let root = Self::build(leaves);
        Self { root }
    }

    fn build(mut children: Vec<RNode<T>>) -> RNode<T> {
        if children.len() == 1 {
            return children.pop().unwrap();
        }
        if children.len() <= CHILDREN_PER_NODE {
            children.sort_by_key(|node| {
                let sp = node.space();
                let mut total = 0i64;
                for p in sp {
                    total += ((p.min + p.max) / 2).abs();
                }
                total
            });
            return RNode::from_children(children);
        }

        let mut min_cost = i64::MAX;
        let mut min_dim = 0usize;
        let mut min_buckets: Option<Vec<RNode<T>>> = None;

        for d in 0..DIMS {
            Self::sort_nodes(&mut children, d, false);
            let buckets = Self::bucketize(&children);
            let total_cost: i64 = buckets.iter().map(|b| Self::cost(b.space())).sum();
            if min_cost > total_cost {
                min_cost = total_cost;
                min_dim = d;
                min_buckets = Some(buckets);
            }
        }

        let mut buckets = min_buckets.unwrap();
        Self::sort_nodes(&mut buckets, min_dim, true);
        let built_children: Vec<RNode<T>> = buckets
            .into_iter()
            .map(|b| match b {
                RNode::SubTree { children, .. } => Self::build(children),
                leaf => leaf,
            })
            .collect();
        RNode::from_children(built_children)
    }

    fn sort_nodes(nodes: &mut [RNode<T>], start_dim: usize, absolute: bool) {
        nodes.sort_by(|a, b| {
            let sa = a.space();
            let sb = b.space();
            for offset in 0..DIMS {
                let d = (start_dim + offset) % DIMS;
                let ca = (sa[d].min + sa[d].max) / 2;
                let cb = (sb[d].min + sb[d].max) / 2;
                let ka = if absolute { ca.abs() } else { ca };
                let kb = if absolute { cb.abs() } else { cb };
                match ka.cmp(&kb) {
                    Ordering::Equal => continue,
                    ord => return ord,
                }
            }
            Ordering::Equal
        });
    }

    fn bucketize(nodes: &[RNode<T>]) -> Vec<RNode<T>> {
        let mut buckets = Vec::new();
        let mut cur = Vec::new();
        let exp = ((nodes.len() as f64 - 0.01).ln() / (CHILDREN_PER_NODE as f64).ln()).floor();
        let expected_children = (CHILDREN_PER_NODE as f64).powf(exp) as usize;
        let expected_children = expected_children.max(1);

        for child in nodes {
            cur.push(child.clone());
            if cur.len() >= expected_children {
                buckets.push(RNode::from_children(std::mem::take(&mut cur)));
            }
        }
        if !cur.is_empty() {
            buckets.push(RNode::from_children(cur));
        }
        buckets
    }

    fn cost(space: &[ClimateParameter; DIMS]) -> i64 {
        let mut total = 0i64;
        for p in space {
            total += (p.max - p.min).abs();
        }
        total
    }

    /// Search nearest biome leaf matching Java `Climate.RTree.search`.
    /// Passing a reusable `last_leaf` mirrors Java's `ThreadLocal<Leaf>` cache for exact tie-breaking!
    pub fn search<'a>(
        &'a self,
        target: TargetPoint,
        last_leaf: &mut Option<(&'a [ClimateParameter; DIMS], T)>,
    ) -> T {
        let target_arr = target.to_array();
        let found = Self::search_node(&self.root, &target_arr, *last_leaf);
        *last_leaf = Some(found);
        found.1
    }

    fn search_node<'a>(
        node: &'a RNode<T>,
        target: &[i64; DIMS],
        candidate: Option<(&'a [ClimateParameter; DIMS], T)>,
    ) -> (&'a [ClimateParameter; DIMS], T) {
        match node {
            RNode::Leaf { space, value } => (space, *value),
            RNode::SubTree { children, .. } => {
                let mut min_dist = match candidate {
                    None => i64::MAX,
                    Some((sp, _)) => space_distance(sp, target),
                };
                let mut closest = candidate;

                for child in children {
                    let child_dist = child.distance(target);
                    if min_dist > child_dist {
                        let leaf = Self::search_node(child, target, closest);
                        let leaf_dist = match child {
                            RNode::Leaf { .. } => child_dist,
                            RNode::SubTree { .. } => space_distance(leaf.0, target),
                        };
                        if min_dist > leaf_dist {
                            min_dist = leaf_dist;
                            closest = Some(leaf);
                        }
                    }
                }

                closest.unwrap()
            }
        }
    }
}

#[inline]
fn space_distance(space: &[ClimateParameter; DIMS], target: &[i64; DIMS]) -> i64 {
    let mut dist = 0i64;
    for i in 0..DIMS {
        let d = space[i].distance(target[i]);
        dist += d * d;
    }
    dist
}

/// Pre-built Overworld biome parameter R-Tree over all 7,594 vanilla Minecraft 26.3 climate entries.
pub struct OverworldBiomeIndex {
    tree: ClimateRTree<u16>,
}

impl OverworldBiomeIndex {
    pub fn build_vanilla() -> Self {
        let entries = OVERWORLD_CLIMATE_ENTRIES
            .iter()
            .map(|e| {
                let space = [
                    ClimateParameter::new(e.params[0].0, e.params[0].1),
                    ClimateParameter::new(e.params[1].0, e.params[1].1),
                    ClimateParameter::new(e.params[2].0, e.params[2].1),
                    ClimateParameter::new(e.params[3].0, e.params[3].1),
                    ClimateParameter::new(e.params[4].0, e.params[4].1),
                    ClimateParameter::new(e.params[5].0, e.params[5].1),
                    ClimateParameter::new(e.params[6].0, e.params[6].1),
                ];
                (space, e.biome_idx)
            })
            .collect();
        Self {
            tree: ClimateRTree::create(entries),
        }
    }

    pub fn find_biome_index(&self, target: TargetPoint) -> u16 {
        let mut cache = None;
        self.tree.search(target, &mut cache)
    }

    pub fn find_biome_name(&self, target: TargetPoint) -> &'static str {
        OVERWORLD_BIOME_NAMES[self.find_biome_index(target) as usize]
    }

    pub fn find_biome_batch(&self, targets: &[TargetPoint]) -> Vec<&'static str> {
        let mut cache = None;
        targets
            .iter()
            .map(|&t| {
                let idx = self.tree.search(t, &mut cache);
                OVERWORLD_BIOME_NAMES[idx as usize]
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_math::{RandomSource, XoroshiroRandomSource};
    use mc_oracle::with_oracle;
    use serde_json::json;

    #[test]
    fn diff_overworld_climate_rtree_1000_targets_against_java_26_3() {
        let index = OverworldBiomeIndex::build_vanilla();
        let mut rng = XoroshiroRandomSource::new(263_2026);
        let mut targets = Vec::with_capacity(1000);
        let mut targets_json = Vec::with_capacity(1000);

        // Include exact boundary points + 995 randomized 6D climate vectors across [-1.5, 1.5]
        let edge_points = [
            TargetPoint::from_floats(0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
            TargetPoint::from_floats(-1.0, -1.0, -1.0, -1.0, 0.0, 0.0),
            TargetPoint::from_floats(1.0, 1.0, 1.0, 1.0, 0.0, 1.0),
            TargetPoint::from_floats(0.2, 0.3, 0.5, -0.4, 1.0, -0.7), // underground lush/dripstone/deep_dark
            TargetPoint::from_floats(-0.45, 0.0, -0.19, 0.05, 0.0, 0.4),
        ];
        for tp in edge_points {
            targets.push(tp);
            targets_json.push(json!([
                tp.temperature,
                tp.humidity,
                tp.continentalness,
                tp.erosion,
                tp.depth,
                tp.weirdness
            ]));
        }
        for _ in 0..995 {
            let depth = if rng.next_int_bounded(5) == 0 {
                rng.next_float() * 1.2_f32
            } else {
                0.0_f32
            };
            let t = (rng.next_float() * 2.4_f32) - 1.2_f32;
            let h = (rng.next_float() * 2.4_f32) - 1.2_f32;
            let c = (rng.next_float() * 2.4_f32) - 1.2_f32;
            let e = (rng.next_float() * 2.4_f32) - 1.2_f32;
            let w = (rng.next_float() * 2.4_f32) - 1.2_f32;
            let tp = TargetPoint::from_floats(t, h, c, e, depth, w);
            targets.push(tp);
            targets_json.push(json!([
                tp.temperature,
                tp.humidity,
                tp.continentalness,
                tp.erosion,
                tp.depth,
                tp.weirdness
            ]));
        }

        let resp = with_oracle(|o| {
            o.call(json!({
                "op": "climate_sample",
                "targets": targets_json
            }))
        })
        .unwrap();
        let java_biomes = resp["biomes"].as_array().unwrap();
        let rust_biomes = index.find_biome_batch(&targets);

        let mut mismatches = 0usize;
        for (i, (&rb, jb_val)) in rust_biomes.iter().zip(java_biomes.iter()).enumerate() {
            let jb = jb_val.as_str().unwrap();
            if rb != jb {
                mismatches += 1;
                if mismatches <= 5 {
                    eprintln!("Mismatch #{i} at {:?}: rust={rb} java={jb}", targets[i]);
                }
            }
        }
        assert_eq!(mismatches, 0, "Expected 0/1000 biome mismatches against Java 26.3!");
    }
}
