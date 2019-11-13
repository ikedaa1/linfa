use ndarray::{s, Array1, Array2, ArrayBase, Axis, Data, DataMut, Ix1, Ix2, Zip};
use ndarray_rand::rand;
use ndarray_rand::rand::Rng;
use ndarray_stats::DeviationExt;
use std::collections::HashMap;
use std::path::PathBuf;
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize)]
/// K-means clustering aims to partition a set of observations into clusters,
/// where each observation belongs to the cluster with the nearest mean.
///
/// The mean of the points within a cluster is called *centroid*.
///
/// Given the set of centroids, you can assign an observation to a cluster
/// choosing the nearest centroid.
///
/// We provide an implementation of the _standard algorithm_, also known as
/// Lloyd's algorithm or naive K-means. Details on the algorithm can be
/// found [here](https://en.wikipedia.org/wiki/K-means_clustering).
pub struct KMeans {
    tolerance: f64,
    max_n_iterations: u64,
    // Only set after `fit` has been called
    centroids: Option<Array2<f64>>,
}

impl KMeans {
    /// K-means is an iterative algorithm: it progressively refines the choice of centroids.
    /// It's guaranteed to converge, even though it might not find the optimal set of centroids
    /// (unfortunately it can get stuck in a local minimum, finding the optimal minimum if NP-hard!).
    ///
    /// There are three steps in the standard algorithm:
    /// - initialisation step: how do we choose our initial set of centroids?
    /// - assignment step: assign each observation to the nearest cluster
    ///                    (minimum distance between the observation and the cluster's centroid);
    /// - update step: recompute the centroid of each cluster.
    ///
    /// The initialisation step is a one-off, done at the very beginning.
    /// Assignment and update are repeated in a loop until convergence is reached (we'll get back
    /// to what this means soon enough).
    ///
    /// `new` lets us configure our training algorithm:
    /// * the training is considered complete if the euclidean distance
    ///   between the old set of centroids and the new set of centroids
    ///   after a training iteration is lower or equal than `tolerance`;
    /// * we exit the training loop when the number of training iterations
    ///   exceeds `max_n_iterations` even if the `tolerance` convergence
    ///   condition has not been met.
    ///
    /// Defaults are provided if `None` is passed as argument:
    /// * `tolerance = 1e-4`;
    /// * `max_n_iterations = 300`.
    pub fn new(tolerance: Option<f64>, max_n_iterations: Option<u64>) -> Self {
        Self {
            tolerance: tolerance.unwrap_or(1e-4),
            max_n_iterations: max_n_iterations.unwrap_or(300),
            centroids: None,
        }
    }
    /// Given an input matrix `observations`, with shape `(n_observations, n_features)`,
    /// `fit` identifies `n_clusters` centroids based on the training data distribution.
    ///
    /// `self` is modified in place (`self.centroids` is mutated), nothing is returned.
    pub fn fit(
        &mut self,
        n_clusters: usize,
        observations: &ArrayBase<impl Data<Elem = f64> + Sync, Ix2>,
        rng: &mut impl Rng
    ) {
        let mut centroids = get_random_centroids(n_clusters, observations, rng);

        let mut has_converged;
        let mut n_iterations = 0;

        let mut memberships = Array1::zeros(observations.dim().0);

        loop {
            update_cluster_memberships(&centroids, observations, &mut memberships);
            let new_centroids = compute_centroids(n_clusters, observations, &memberships);

            let distance = centroids
                .sq_l2_dist(&new_centroids)
                .expect("Failed to compute distance");
            has_converged = distance < self.tolerance || n_iterations > self.max_n_iterations;

            centroids = new_centroids;
            n_iterations += 1;

            if has_converged {
                break;
            }
        }

        self.centroids = Some(centroids);
    }

    /// Given an input matrix `observations`, with shape `(n_observations, n_features)`,
    /// `predict` returns, for each observation, the index of the closest cluster/centroid.
    ///
    /// You can retrieve the centroid associated to an index using the
    /// [`centroids` method](#method.centroids) (e.g. `self.centroids()[cluster_index]`).
    pub fn predict(&self, observations: &ArrayBase<impl Data<Elem = f64>, Ix2>) -> Array1<usize> {
        compute_cluster_memberships(
            self.centroids
                .as_ref()
                .expect("The model has to be fitted before calling predict!"),
            observations,
        )
    }

    pub fn save(&self, path: PathBuf) -> std::io::Result<()> {
        std::fs::write(
            path,
           serde_json::to_string(&self)?
        )
    }

    pub fn load(path: PathBuf) -> Result<Self, anyhow::Error> {
        let reader = &std::fs::File::open(path)?;
        let model = serde_json::from_reader(reader)?;
        Ok(model)
    }

    /// Return the set of centroids as a 2-dimensional matrix with shape
    /// `(n_centroids, n_features)`.
    pub fn centroids(&self) -> Option<&Array2<f64>> {
        self.centroids.as_ref()
    }
}

fn compute_centroids(
    n_clusters: usize,
    observations: &ArrayBase<impl Data<Elem = f64>, Ix2>,
    cluster_memberships: &ArrayBase<impl Data<Elem = usize>, Ix1>,
) -> Array2<f64> {
    let centroids_hashmap = compute_centroids_hashmap(&observations, &cluster_memberships);
    let (_, n_features) = observations.dim();

    let mut centroids: Array2<f64> = Array2::zeros((n_clusters, n_features));
    for (centroid_index, centroid) in centroids_hashmap.into_iter() {
        centroids
            .slice_mut(s![centroid_index, ..])
            .assign(&centroid.current_mean);
    }
    centroids
}

/// Iterate over our observations and capture in a HashMap the new centroids.
/// The HashMap is a (cluster_index => new centroid) mapping.
fn compute_centroids_hashmap(
    // (n_observations, n_features)
    observations: &ArrayBase<impl Data<Elem = f64>, Ix2>,
    // (n_observations,)
    cluster_memberships: &ArrayBase<impl Data<Elem = usize>, Ix1>,
) -> HashMap<usize, IncrementalMean> {
    let mut new_centroids: HashMap<usize, IncrementalMean> = HashMap::new();
    Zip::from(observations.genrows())
        .and(cluster_memberships)
        .apply(|observation, cluster_membership| {
            if let Some(incremental_mean) = new_centroids.get_mut(cluster_membership) {
                incremental_mean.update(&observation);
            } else {
                new_centroids.insert(
                    *cluster_membership,
                    IncrementalMean::new(observation.to_owned()),
                );
            }
        });
    new_centroids
}

struct IncrementalMean {
    pub current_mean: Array1<f64>,
    pub n_observations: usize,
}

impl IncrementalMean {
    fn new(first_observation: Array1<f64>) -> Self {
        Self {
            current_mean: first_observation,
            n_observations: 1,
        }
    }

    fn update(&mut self, new_observation: &ArrayBase<impl Data<Elem = f64>, Ix1>) {
        self.n_observations += 1;
        let shift =
            (new_observation - &self.current_mean).mapv_into(|x| x / self.n_observations as f64);
        self.current_mean += &shift;
    }
}

fn update_cluster_memberships(
    centroids: &ArrayBase<impl Data<Elem = f64> + Sync, Ix2>,
    observations: &ArrayBase<impl Data<Elem = f64> + Sync, Ix2>,
    cluster_memberships: &mut ArrayBase<impl DataMut<Elem = usize>, Ix1>,
) {
    Zip::from(observations.axis_iter(Axis(0)))
        .and(cluster_memberships)
        .par_apply(|observation, cluster_membership| {
            *cluster_membership = closest_centroid(&centroids, &observation)
        });
}

fn compute_cluster_memberships(
    centroids: &ArrayBase<impl Data<Elem = f64>, Ix2>,
    observations: &ArrayBase<impl Data<Elem = f64>, Ix2>,
) -> Array1<usize> {
    observations.map_axis(Axis(1), |observation| {
        closest_centroid(&centroids, &observation)
    })
}

fn closest_centroid(
    centroids: &ArrayBase<impl Data<Elem = f64>, Ix2>,
    observation: &ArrayBase<impl Data<Elem = f64>, Ix1>,
) -> usize {
    let mut iterator = centroids.genrows().into_iter().peekable();

    let first_centroid = iterator
        .peek()
        .expect("There has to be at least one centroid");
    let (mut closest_index, mut minimum_distance) = (
        0,
        first_centroid
            .sq_l2_dist(&observation)
            .expect("Failed to compute distance"),
    );

    for (centroid_index, centroid) in iterator.enumerate() {
        let distance = centroid
            .sq_l2_dist(&observation)
            .expect("Failed to compute distance");
        if distance < minimum_distance {
            closest_index = centroid_index;
            minimum_distance = distance;
        }
    }
    closest_index
}

fn get_random_centroids<S>(
    n_clusters: usize,
    observations: &ArrayBase<S, Ix2>,
    rng: &mut impl Rng,
) -> Array2<f64>
where
    S: Data<Elem = f64>,
{
    let (n_samples, _) = observations.dim();
    let indices = rand::seq::index::sample(rng, n_samples, n_clusters).into_vec();
    observations.select(Axis(0), &indices)
}
