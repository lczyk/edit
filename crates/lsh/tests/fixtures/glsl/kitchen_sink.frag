#version 330 core
out vec4 color;
uniform float density_alpha;

void main() {
    // Discard each point outside of the circle radius
    // https://stackoverflow.com/a/27099691/2531987
    vec2 circCoord = 2.0 * gl_PointCoord - 1.0;
    if (dot(circCoord, circCoord) > 1.0) { discard; }

    // Additive accumulation (blended with GL_ONE,GL_ONE): each point adds density_alpha
    // to .r, so density grows UNBOUNDED instead of saturating at 1.0. Consumers tone-map
    // it on read -- the physics with 1-exp(-d) (~= the old saturating field), the screen
    // with a log map that keeps the dense cores' internal gradient (no flat clipping).
    color = vec4(density_alpha, 0.0f, 0.0f, 1.0f);
}