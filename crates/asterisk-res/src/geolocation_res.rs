//! Geolocation framework (PIDF-LO XML generation).
//!
//! Port of `res/res_geolocation.c` and `res/res_geolocation/`. Provides
//! the geolocation framework for generating PIDF-LO (Presence Information
//! Data Format - Location Object) XML documents containing civic addresses,
//! GML shapes, and geolocation profiles per RFC 4119 / RFC 5491.



// ---------------------------------------------------------------------------
// Civic address elements (RFC 4119 / RFC 5139)
// ---------------------------------------------------------------------------

/// A civic address location (RFC 5139 elements).
#[derive(Debug, Clone, Default)]
pub struct CivicAddress {
    /// Country (ISO 3166-1 alpha-2).
    pub country: Option<String>,
    /// State/province (A1).
    pub a1: Option<String>,
    /// County/region (A2).
    pub a2: Option<String>,
    /// City (A3).
    pub a3: Option<String>,
    /// City division/borough (A4).
    pub a4: Option<String>,
    /// Street name (A6) / leading street direction (PRD) / trailing (POD).
    pub a6: Option<String>,
    /// House number (HNO).
    pub hno: Option<String>,
    /// House number suffix (HNS).
    pub hns: Option<String>,
    /// Floor (FLR).
    pub flr: Option<String>,
    /// Room/suite (ROOM).
    pub room: Option<String>,
    /// Postal/ZIP code (PC).
    pub pc: Option<String>,
    /// Location name (NAM).
    pub nam: Option<String>,
}

impl CivicAddress {
    pub fn new() -> Self {
        Self::default()
    }

    /// Generate the civic address XML fragment for a PIDF-LO document.
    pub fn to_xml(&self) -> String {
        let mut xml = String::new();
        xml.push_str("<ca:civicAddress xmlns:ca=\"urn:ietf:params:xml:ns:pidf:geopriv10:civicAddr\">\n");
        if let Some(ref v) = self.country {
            xml.push_str(&format!("  <ca:country>{}</ca:country>\n", v));
        }
        if let Some(ref v) = self.a1 {
            xml.push_str(&format!("  <ca:A1>{}</ca:A1>\n", v));
        }
        if let Some(ref v) = self.a2 {
            xml.push_str(&format!("  <ca:A2>{}</ca:A2>\n", v));
        }
        if let Some(ref v) = self.a3 {
            xml.push_str(&format!("  <ca:A3>{}</ca:A3>\n", v));
        }
        if let Some(ref v) = self.a4 {
            xml.push_str(&format!("  <ca:A4>{}</ca:A4>\n", v));
        }
        if let Some(ref v) = self.a6 {
            xml.push_str(&format!("  <ca:A6>{}</ca:A6>\n", v));
        }
        if let Some(ref v) = self.hno {
            xml.push_str(&format!("  <ca:HNO>{}</ca:HNO>\n", v));
        }
        if let Some(ref v) = self.flr {
            xml.push_str(&format!("  <ca:FLR>{}</ca:FLR>\n", v));
        }
        if let Some(ref v) = self.room {
            xml.push_str(&format!("  <ca:ROOM>{}</ca:ROOM>\n", v));
        }
        if let Some(ref v) = self.pc {
            xml.push_str(&format!("  <ca:PC>{}</ca:PC>\n", v));
        }
        if let Some(ref v) = self.nam {
            xml.push_str(&format!("  <ca:NAM>{}</ca:NAM>\n", v));
        }
        xml.push_str("</ca:civicAddress>\n");
        xml
    }
}

// ---------------------------------------------------------------------------
// GML shapes (RFC 5491)
// ---------------------------------------------------------------------------

/// Geographic coordinates.
#[derive(Debug, Clone, Copy)]
pub struct GeoPoint {
    pub latitude: f64,
    pub longitude: f64,
    /// Altitude in meters (optional).
    pub altitude: Option<f64>,
}

impl GeoPoint {
    pub fn new(latitude: f64, longitude: f64) -> Self {
        Self {
            latitude,
            longitude,
            altitude: None,
        }
    }

    pub fn with_altitude(mut self, altitude: f64) -> Self {
        self.altitude = Some(altitude);
        self
    }

    /// Format as GML pos string.
    pub fn to_pos(&self) -> String {
        match self.altitude {
            Some(alt) => format!("{} {} {}", self.latitude, self.longitude, alt),
            None => format!("{} {}", self.latitude, self.longitude),
        }
    }
}

/// GML shape types for location representation.
#[derive(Debug, Clone)]
pub enum GmlShape {
    /// A single point.
    Point(GeoPoint),
    /// A circle defined by center and radius in meters.
    Circle { center: GeoPoint, radius: f64 },
    /// An ellipse.
    Ellipse {
        center: GeoPoint,
        semi_major: f64,
        semi_minor: f64,
        orientation: f64,
    },
    /// A polygon defined by a list of vertices.
    Polygon(Vec<GeoPoint>),
}

impl GmlShape {
    /// Generate GML XML fragment.
    pub fn to_xml(&self) -> String {
        match self {
            Self::Point(point) => {
                format!(
                    "<gml:Point xmlns:gml=\"http://www.opengis.net/gml\" srsName=\"urn:ogc:def:crs:EPSG::4326\">\n  <gml:pos>{}</gml:pos>\n</gml:Point>\n",
                    point.to_pos()
                )
            }
            Self::Circle { center, radius } => {
                format!(
                    "<gs:Circle xmlns:gs=\"http://www.opengis.net/pidflo/1.0\" xmlns:gml=\"http://www.opengis.net/gml\" srsName=\"urn:ogc:def:crs:EPSG::4326\">\n  <gml:pos>{}</gml:pos>\n  <gs:radius uom=\"urn:ogc:def:uom:EPSG::9001\">{}</gs:radius>\n</gs:Circle>\n",
                    center.to_pos(), radius
                )
            }
            Self::Ellipse { center, semi_major, semi_minor, orientation } => {
                format!(
                    "<gs:Ellipse xmlns:gs=\"http://www.opengis.net/pidflo/1.0\" xmlns:gml=\"http://www.opengis.net/gml\" srsName=\"urn:ogc:def:crs:EPSG::4326\">\n  <gml:pos>{}</gml:pos>\n  <gs:semiMajorAxis uom=\"urn:ogc:def:uom:EPSG::9001\">{}</gs:semiMajorAxis>\n  <gs:semiMinorAxis uom=\"urn:ogc:def:uom:EPSG::9001\">{}</gs:semiMinorAxis>\n  <gs:orientation uom=\"urn:ogc:def:uom:EPSG::9102\">{}</gs:orientation>\n</gs:Ellipse>\n",
                    center.to_pos(), semi_major, semi_minor, orientation
                )
            }
            Self::Polygon(points) => {
                let mut xml = String::new();
                xml.push_str("<gml:Polygon xmlns:gml=\"http://www.opengis.net/gml\" srsName=\"urn:ogc:def:crs:EPSG::4326\">\n");
                xml.push_str("  <gml:exterior>\n    <gml:LinearRing>\n");
                for p in points {
                    xml.push_str(&format!("      <gml:pos>{}</gml:pos>\n", p.to_pos()));
                }
                xml.push_str("    </gml:LinearRing>\n  </gml:exterior>\n</gml:Polygon>\n");
                xml
            }
        }
    }
}

// ---------------------------------------------------------------------------
// PIDF-LO document
// ---------------------------------------------------------------------------

/// Location format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocationFormat {
    /// Civic address.
    Civic,
    /// GML geodetic shape.
    Gml,
}

/// A PIDF-LO (Presence Information Data Format - Location Object) document.
#[derive(Debug, Clone)]
pub struct PidfLoDocument {
    /// Entity URI (presentity).
    pub entity: String,
    /// Civic address (if civic format).
    pub civic: Option<CivicAddress>,
    /// GML shape (if geodetic format).
    pub gml: Option<GmlShape>,
    /// Retention expiry hint.
    pub retention_expiry: Option<String>,
}

impl PidfLoDocument {
    pub fn new_civic(entity: &str, civic: CivicAddress) -> Self {
        Self {
            entity: entity.to_string(),
            civic: Some(civic),
            gml: None,
            retention_expiry: None,
        }
    }

    pub fn new_gml(entity: &str, shape: GmlShape) -> Self {
        Self {
            entity: entity.to_string(),
            civic: None,
            gml: Some(shape),
            retention_expiry: None,
        }
    }

    /// Generate the full PIDF-LO XML document.
    pub fn to_xml(&self) -> String {
        let mut xml = String::new();
        xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        xml.push_str(&format!(
            "<presence xmlns=\"urn:ietf:params:xml:ns:pidf\" entity=\"{}\">\n",
            self.entity
        ));
        xml.push_str("  <tuple id=\"location\">\n");
        xml.push_str("    <status>\n");
        xml.push_str("      <geopriv xmlns=\"urn:ietf:params:xml:ns:pidf:geopriv10\">\n");
        xml.push_str("        <location-info>\n");

        if let Some(ref civic) = self.civic {
            for line in civic.to_xml().lines() {
                xml.push_str(&format!("          {}\n", line));
            }
        }
        if let Some(ref gml) = self.gml {
            for line in gml.to_xml().lines() {
                xml.push_str(&format!("          {}\n", line));
            }
        }

        xml.push_str("        </location-info>\n");
        xml.push_str("      </geopriv>\n");
        xml.push_str("    </status>\n");
        xml.push_str("  </tuple>\n");
        xml.push_str("</presence>\n");
        xml
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_civic_address() {
        let mut addr = CivicAddress::new();
        addr.country = Some("US".to_string());
        addr.a1 = Some("CA".to_string());
        addr.a3 = Some("San Francisco".to_string());
        addr.a6 = Some("Market St".to_string());
        addr.hno = Some("123".to_string());

        let xml = addr.to_xml();
        assert!(xml.contains("civicAddress"));
        assert!(xml.contains("<ca:country>US</ca:country>"));
        assert!(xml.contains("<ca:A3>San Francisco</ca:A3>"));
    }

    #[test]
    fn test_geo_point() {
        let point = GeoPoint::new(37.7749, -122.4194);
        assert_eq!(point.to_pos(), "37.7749 -122.4194");

        let point3d = point.with_altitude(10.0);
        assert_eq!(point3d.to_pos(), "37.7749 -122.4194 10");
    }

    #[test]
    fn test_gml_point() {
        let shape = GmlShape::Point(GeoPoint::new(37.7749, -122.4194));
        let xml = shape.to_xml();
        assert!(xml.contains("gml:Point"));
        assert!(xml.contains("37.7749 -122.4194"));
    }

    #[test]
    fn test_gml_circle() {
        let shape = GmlShape::Circle {
            center: GeoPoint::new(37.7749, -122.4194),
            radius: 100.0,
        };
        let xml = shape.to_xml();
        assert!(xml.contains("gs:Circle"));
        assert!(xml.contains("gs:radius"));
    }

    #[test]
    fn test_pidf_lo_civic() {
        let mut addr = CivicAddress::new();
        addr.country = Some("US".to_string());
        let doc = PidfLoDocument::new_civic("sip:alice@example.com", addr);
        let xml = doc.to_xml();
        assert!(xml.contains("presence"));
        assert!(xml.contains("geopriv"));
        assert!(xml.contains("civicAddress"));
    }

    #[test]
    fn test_pidf_lo_gml() {
        let shape = GmlShape::Point(GeoPoint::new(37.0, -122.0));
        let doc = PidfLoDocument::new_gml("sip:alice@example.com", shape);
        let xml = doc.to_xml();
        assert!(xml.contains("gml:Point"));
    }
}
