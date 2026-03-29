#[cfg(test)]
mod tests {
    use crate::model::*;
    use crate::solver::corridor::route_corridors;
    use crate::solver::layout::solve_layout;

    #[test]
    fn test_k4_corridors() {
        // Create 4 rooms in a graph
        let mut graph = DungeonGraph::new();
        let r1 = Room::new("Room 1".into());
        let r2 = Room::new("Room 2".into());
        let r3 = Room::new("Room 3".into());
        let r4 = Room::new("Room 4".into());
        let ids: Vec<String> = vec![r1.id.clone(), r2.id.clone(), r3.id.clone(), r4.id.clone()];
        graph.add_room(r1);
        graph.add_room(r2);
        graph.add_room(r3);
        graph.add_room(r4);

        // Fully connect: 6 edges
        let pairs = [(0,1),(0,2),(0,3),(1,2),(1,3),(2,3)];
        for (a, b) in &pairs {
            graph.add_connection(ids[*a].clone(), ids[*b].clone(), Connection::new(ConnectionType::Door));
        }

        // Solve layout
        // Use gap=4 to ensure rooms have corridor space between them
        let layout = solve_layout(&graph, 4).expect("layout should solve");
        
        println!("Room positions:");
        for rl in &layout.rooms {
            let label = graph.room_by_id(&rl.room_id).unwrap().label.clone();
            println!("  {} at ({}, {}) size {}x{}", label, rl.x, rl.y, rl.width, rl.height);
        }

        // Route corridors
        let corridors = route_corridors(&graph, &layout);

        println!("\nCorridors: {} / 6 expected", corridors.len());
        for c in &corridors {
            let edge = graph.connections.iter().find(|e| e.connection.id == c.connection_id).unwrap();
            let src = graph.room_by_id(&edge.source_room_id).unwrap().label.clone();
            let tgt = graph.room_by_id(&edge.target_room_id).unwrap().label.clone();
            let wps: Vec<String> = c.waypoints.iter().map(|w| format!("({},{})", w.x, w.y)).collect();
            println!("  {} -> {}: {} invalid={} path={}", 
                src, tgt, 
                if c.invalid { "FAIL" } else { "OK  " },
                c.invalid,
                wps.join(" -> "));
        }

        let valid_count = corridors.iter().filter(|c| !c.invalid).count();
        println!("\nValid: {}/6", valid_count);
        
        // Check what's blocking
        if valid_count < 6 {
            println!("\nDebugging blocked corridors...");
            for c in &corridors {
                if !c.invalid { continue; }
                let edge = graph.connections.iter().find(|e| e.connection.id == c.connection_id).unwrap();
                let src = graph.room_by_id(&edge.source_room_id).unwrap().label.clone();
                let tgt = graph.room_by_id(&edge.target_room_id).unwrap().label.clone();
                println!("  BLOCKED: {} -> {}", src, tgt);
            }
        }

        assert_eq!(corridors.len(), 6, "Should have 6 corridors for K4");
        assert_eq!(valid_count, 6, "All 6 corridors must be valid for K4");

        // Verify no actual geometric overlap between any pair of corridors
        let mut full_layout = layout.clone();
        full_layout.corridors = corridors;
        full_layout.recheck_corridor_overlaps();
        for c in &full_layout.corridors {
            let edge = graph.connections.iter().find(|e| e.connection.id == c.connection_id).unwrap();
            let src = graph.room_by_id(&edge.source_room_id).unwrap().label.clone();
            let tgt = graph.room_by_id(&edge.target_room_id).unwrap().label.clone();
            assert!(!c.invalid, "Corridor {} -> {} overlaps another corridor!", src, tgt);
        }
    }
}
