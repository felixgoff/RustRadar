//! Exercises the main endpoints against the live API.
//!
//! ```sh
//! cargo run -p fr24 --example smoke
//! ```

use std::time::Instant;

use fr24::Fr24;
use fr24::bbox::FRANCE_UIR;
use fr24::grpc::{
    FlightDetailsParams, FlightDetailsRecord, FlightRecord, FollowFlightParams, LiveFeedParams,
    NearbyFlightRecord, NearestFlightsParams, TopFlightRecord, TopFlightsParams,
};
use fr24::json::{FindParams, FlightListParams, FlightListQuery};
use futures_util::StreamExt;

#[tokio::main]
async fn main() -> fr24::Result<()> {
    let mut fr24 = Fr24::new()?;
    let auth = fr24.login(None).await?;
    println!("authenticated: {}", auth.is_some());

    let feed = fr24.live_feed(&LiveFeedParams::new(FRANCE_UIR)).await?;
    println!(
        "\n# live feed (France UIR): {} flights",
        feed.flights_list.len()
    );
    if let Some(f) = feed.flights_list.first() {
        println!("{:#?}", FlightRecord::from(f));
    }

    let start = Instant::now();
    let template = LiveFeedParams::new(FRANCE_UIR);
    let world = fr24.live_feed_world(&template, 8).await;
    println!(
        "
# live feed (world): {} flights, {} requests, {} errors, {:?}",
        world.flights.len(),
        world.requests,
        world.errors.len(),
        start.elapsed()
    );
    for (b, e) in &world.errors {
        println!("  {b:?}: {e}");
    }

    let nearest = fr24
        .nearest_flights(&NearestFlightsParams::new(51.47, -0.4543))
        .await?;
    println!("\n# nearest flights (LHR): {}", nearest.flights_list.len());
    if let Some(nf) = nearest.flights_list.first() {
        let r = NearbyFlightRecord::from(nf);
        println!(
            "{} {} {}m",
            r.flight.callsign, r.flight.typecode, r.distance
        );
    }

    let top = fr24.top_flights(&TopFlightsParams::default()).await?;
    println!("\n# top flights");
    for ff in &top.scoreboard_list {
        let r = TopFlightRecord::from(ff);
        println!(
            "{:>8x} {:<8} {:>5} {}-{} {}",
            r.flight_id, r.callsign, r.live_clicks, r.from_iata, r.to_iata, r.full_description
        );
    }

    if let Some(ff) = top.scoreboard_list.first() {
        let details = fr24
            .flight_details(&FlightDetailsParams::new(ff.flight_id))
            .await?;
        let r = FlightDetailsRecord::from(&details);
        let (aircraft, flight, progress) =
            (r.aircraft.unwrap(), r.flight.unwrap(), r.progress.unwrap());
        println!(
            "
# flight details {:x}: {} {} {} alt={} trail={} stage={}",
            flight.flightid,
            flight.callsign,
            aircraft.reg,
            aircraft.full_description,
            flight.altitude,
            r.trail.len(),
            progress.flight_stage
        );
    }

    // follow a cruising flight, which updates every few seconds
    if let Some(f) = feed.flights_list.iter().find(|f| f.alt > 30_000) {
        println!("\n# follow flight {:x} ({})", f.flightid, f.callsign);
        let start = Instant::now();
        let stream = fr24
            .follow_flight(&FollowFlightParams::new(f.flightid as u32))
            .await?;
        let mut stream = std::pin::pin!(stream.take(3));
        while let Some(msg) = stream.next().await {
            let msg = msg?;
            let info = msg.flight_info.unwrap_or_default();
            println!(
                "  +{:.1?} {} lat={} lon={} alt={} trail={}",
                start.elapsed(),
                info.callsign,
                info.lat,
                info.lon,
                info.alt,
                msg.flight_trail_list.len()
            );
        }
    }

    let find = fr24.find(&FindParams::new("CX251")).await?;
    println!("\n# find CX251: {} results", find.results.len());
    for e in find.results.iter().take(5) {
        println!("  {} {}", e.id, e.label);
    }

    let list = fr24
        .flight_list(&FlightListParams::new(FlightListQuery::Flight(
            "CX251".into(),
        )))
        .await?;
    let n = list["result"]["response"]["data"]
        .as_array()
        .map_or(0, |a| a.len());
    println!("\n# flight list CX251: {n} rows");

    Ok(())
}
