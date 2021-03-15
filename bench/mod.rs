use bustle::*;
use std::env;
use std::fmt::Debug;
use std::io::*;
use std::fs::File;
mod arc_mutex_std;
mod arc_rwlock_std;
mod chashmap;
mod lfmap;

fn main() {
    tracing_subscriber::fmt::init();
    // Workload::new(8, Mix::insert_heavy()).run::<lfmap::TestTable>();
    // Workload::new(16, Mix::uniform()).run::<lfmap::TestTable>();
    // Workload::new(126, Mix::read_heavy()).run::<lfmap::TestTable>();
    // Workload::new(1, Mix::uniform()).run::<lfmap::TestTable>();
    test_lfmap();
    test_chashmap();
    test_rwlock_std();
    test_mutex_std();
}

fn get_task_name() -> String {
    env::args().next().unwrap()
}

fn test_lfmap() {
    let args: Vec<String> = env::args().collect();
    run_and_record::<lfmap::TestTable>(&get_task_name(), "lock-free map");
}

fn test_rwlock_std() {
    let args: Vec<String> = env::args().collect();
    run_and_record::<arc_rwlock_std::Table<u64>>(&get_task_name(), "rwlock std map");
}

fn test_mutex_std() {
    let args: Vec<String> = env::args().collect();
    run_and_record::<arc_mutex_std::Table<u64>>(&get_task_name(), "mutex std map");
}

fn test_chashmap() {
    let args: Vec<String> = env::args().collect();
    run_and_record::<chashmap::Table<u64>>(&get_task_name(), "CHashmap");
}

fn run_and_record<'a, 'b, T: Collection>(task: &'b str, name: &'a str)
where
    <T::Handle as CollectionHandle>::Key: Send + Debug,
{
    println!("Testing {}", name);
    println!("Insert heavy");
    let insert_measure = run_and_measure_mix::<T>(Mix::insert_heavy());
    println!("Read heavy");
    let read_measure = run_and_measure_mix::<T>(Mix::read_heavy());
    println!("Uniform");
    let uniform_measure = run_and_measure_mix::<T>(Mix::uniform());
    write_measures(&format!("{}_{}_insertion.csv", task, name), &insert_measure);
    write_measures(&format!("{}_{}_read.csv", task, name), &read_measure);
    write_measures(&format!("{}_{}_uniform.csv", task, name), &uniform_measure);
}

fn run_and_measure_mix<T: Collection>(mix: Mix) -> Vec<Measurement>
where
    <T::Handle as CollectionHandle>::Key: Send + Debug,
{
    (1..=num_cpus::get())
        .map(|n| {
            let m = run_and_measure::<T>(n, mix);
            println!(
                "Completed with threads {}, ops {}, spent {:?}, throughput {}, latency {:?}",
                n, m.total_ops, m.spent, m.throughput, m.latency
            );
            m
        })
        .collect()
}

fn run_and_measure<T: Collection>(threads: usize, mix: Mix) -> Measurement
where
    <T::Handle as CollectionHandle>::Key: Send + Debug,
{
    let workload = Workload::new(threads, mix);
    workload.run_silently::<T>()
}

fn write_measures<'a>(name: &'a str, measures: &[Measurement]) {
    let file = File::create(name).unwrap();
    let mut file = LineWriter::new(file);
    for (n, m) in measures.iter().enumerate() {
        file.write_all(format!(
            "{}\t{}\t{}\t{}\t{}\n",
            n,
            m.total_ops,
            m.spent.as_nanos(),
            m.throughput,
            m.latency.as_nanos()
        ).as_bytes());
    }
}