use crate::*;
use std::time::Duration;

#[test]
fn test_point_operations() {
    let p1 = Point::new(0.0, 0.0);
    let p2 = Point::new(3.0, 4.0);

    assert_eq!(p1.distance_to(&p2), 5.0);
    assert_eq!(p2.distance_to(&p1), 5.0);
}

#[test]
fn test_input_data_creation() {
    let point = Point::new(100.0, 200.0);
    let timestamp = Duration::from_millis(100);

    let input = InputData::new(point, timestamp)
        .with_pressure(0.5)
        .with_tilt(Point::new(0.1, 0.2))
        .with_orientation(45.0);

    assert_eq!(input.position.x, 100.0);
    assert_eq!(input.pressure, Some(0.5));
    assert_eq!(input.tilt, Some(Point::new(0.1, 0.2)));
    assert_eq!(input.orientation, Some(45.0));
}

#[test]
fn test_stroke_operations() {
    let mut stroke = Stroke::new();
    assert!(stroke.is_empty());

    stroke.add_point(InputData::new(
        Point::new(0.0, 0.0),
        Duration::from_millis(0),
    ));
    stroke.add_point(InputData::new(
        Point::new(3.0, 4.0),
        Duration::from_millis(10),
    ));

    assert_eq!(stroke.len(), 2);
    assert_eq!(stroke.total_distance(), 5.0);
    assert_eq!(stroke.duration(), Some(Duration::from_millis(10)));
    assert_eq!(stroke.average_velocity(), Some(500.0)); // 5.0 distance / 0.01 seconds
}

#[test]
fn test_median_filter() {
    let mut filter = MedianFilter::new(3);

    let input1 = InputData::new(Point::new(10.0, 10.0), Duration::from_millis(0));
    let input2 = InputData::new(Point::new(20.0, 20.0), Duration::from_millis(1));
    let input3 = InputData::new(Point::new(30.0, 30.0), Duration::from_millis(2));

    // First input should be passed through
    let filtered1 = filter.filter(&input1);
    assert!(filtered1.is_some());
    assert_eq!(filtered1.unwrap().position.x, 10.0);

    // Second input (median of [10, 20] is 10)
    let filtered2 = filter.filter(&input2);
    assert!(filtered2.is_some());
    assert_eq!(filtered2.unwrap().position.x, 10.0);

    // Third input (median of [10, 20, 30] is 20)
    let filtered3 = filter.filter(&input3);
    assert!(filtered3.is_some());
    assert_eq!(filtered3.unwrap().position.x, 20.0);
}

#[test]
fn test_velocity_filter() {
    let mut filter = VelocityFilter::new(1000.0); // max 1000 pixels per second

    let input1 = InputData::new(Point::new(0.0, 0.0), Duration::from_millis(0));
    let input2 = InputData::new(Point::new(10.0, 0.0), Duration::from_millis(10)); // 1000 px/s at threshold
    let input3 = InputData::new(Point::new(30.0, 0.0), Duration::from_millis(20)); // 2000 px/s exceeds threshold

    assert!(filter.filter(&input1).is_some());
    assert!(filter.filter(&input2).is_some());
    assert!(filter.filter(&input3).is_none()); // Should be filtered out
}

#[test]
fn test_input_manager() {
    let config = InputConfig::default();
    let mut manager = InputManager::new(config);

    let raw_input = RawInput::new(100.0, 150.0)
        .with_pressure(0.5)
        .with_timestamp(Duration::from_millis(100));

    assert!(manager.add_input(raw_input).is_ok());
    assert_eq!(manager.current_stroke().len(), 1);

    let completed_stroke = manager.finish_stroke();
    assert_eq!(completed_stroke.len(), 1);
    assert!(manager.current_stroke().is_empty());
    assert_eq!(manager.stroke_count(), 1);
}

#[test]
fn test_distance_sampling() {
    let sampler = DistanceSampling::new(5.0);

    let input1 = InputData::new(Point::new(0.0, 0.0), Duration::from_millis(0));
    let input2 = InputData::new(Point::new(3.0, 0.0), Duration::from_millis(1));
    let input3 = InputData::new(Point::new(10.0, 0.0), Duration::from_millis(2));

    assert!(sampler.should_sample(&input1, None));
    assert!(!sampler.should_sample(&input2, Some(&input1))); // distance = 3 < 5
    assert!(sampler.should_sample(&input3, Some(&input1))); // distance = 10 >= 5
}

#[test]
fn test_bounds() {
    let mut stroke = Stroke::new();
    stroke.add_point(InputData::new(
        Point::new(0.0, 0.0),
        Duration::from_millis(0),
    ));
    stroke.add_point(InputData::new(
        Point::new(10.0, 20.0),
        Duration::from_millis(1),
    ));
    stroke.add_point(InputData::new(
        Point::new(-5.0, 15.0),
        Duration::from_millis(2),
    ));

    let bounds = Bounds::from_stroke(&stroke);
    assert_eq!(bounds.min_x, -5.0);
    assert_eq!(bounds.min_y, 0.0);
    assert_eq!(bounds.max_x, 10.0);
    assert_eq!(bounds.max_y, 20.0);
    assert_eq!(bounds.width(), 15.0);
    assert_eq!(bounds.height(), 20.0);
    assert!(bounds.contains(&Point::new(5.0, 10.0)));
    assert!(!bounds.contains(&Point::new(15.0, 10.0)));
}

#[test]
fn test_stroke_metrics() {
    let metrics = StrokeMetrics {
        smoothness: 0.8,
        consistency: 0.9,
        completeness: 0.7,
        confidence: 0.85,
    };

    assert_eq!(metrics.overall_quality(), (0.8 + 0.9 + 0.7 + 0.85) / 4.0);
}
