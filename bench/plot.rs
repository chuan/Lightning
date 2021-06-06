use super::PerfPlotData;
use bustle::*;
use plotters::prelude::*;
use std::collections::HashMap;

pub fn draw_perf_plots(data: PerfPlotData) {
    let mut data_sets = HashMap::new();
    for (ds, ds_data) in &data {
        for (cnt, cnt_data) in ds_data {
            for (work_load, work_load_data) in cnt_data {
                data_sets
                    .entry((ds, cnt))
                    .or_insert(vec![])
                    .push((*work_load, work_load_data));
                data_sets
                    .entry((ds, work_load))
                    .or_insert(vec![])
                    .push((*cnt, work_load_data));
                data_sets
                    .entry((work_load, cnt))
                    .or_insert(vec![])
                    .push((*ds, work_load_data));
            }
        }
    }
    for ((s1, s2), data) in data_sets {
        let title = if *s2 == "*" {
            s1.to_string()
        } else {
            format!("{} - {}", s1, s2)
        };
        plot_perf(&title, data).unwrap();
    }
}

pub fn plot_perf(
    title: &String,
    data: Vec<(&'static str, &Vec<(usize, Measurement)>)>,
) -> Result<(), Box<dyn std::error::Error>> {
    let x_scale = data
        .iter()
        .map(|(_ser_str, measures)| measures.iter().map(|(threads, _)| threads).max().unwrap())
        .max()
        .unwrap();
    let y_scale = data
        .iter()
        .map(|(_ser_str, measures)| {
            measures
                .iter()
                .map(|(_, m)| m.throughput as usize + 10)
                .max()
                .unwrap()
        })
        .max()
        .unwrap() as f64;
    let file_name = &format!("{}.png", title);
    let root_area = BitMapBackend::new(file_name, (640, 480)).into_drawing_area();
    root_area.fill(&WHITE)?;
    let mut chart = ChartBuilder::on(&root_area)
        .margin(20)
        .caption(title, ("sans-serif", 40).into_font())
        .x_label_area_size(30)
        .y_label_area_size(30)
        .build_cartesian_2d(1..*x_scale, 0.0..y_scale)?;
    chart
        .configure_mesh()
        .x_desc("Threads")
        .y_desc("Throughput")
        .y_label_formatter(&|y| format!("{:+e}", y))
        .draw()?;
    for (i, (title, data)) in data.iter().enumerate() {
        let color = Palette99::pick(i).mix(0.9);
        chart
            .draw_series(LineSeries::new(
                data.iter().map(|(t, m)| (*t, m.throughput)),
                color.stroke_width(2),
            ))?
            .label(*title)
            .legend(move |(x, y)| Rectangle::new([(x, y - 5), (x + 10, y + 5)], color.filled()));
    }
    chart
        .configure_series_labels()
        .background_style(&WHITE.mix(0.8))
        .border_style(&BLACK)
        .position(SeriesLabelPosition::MiddleRight)
        .draw()?;
    Ok(())
}