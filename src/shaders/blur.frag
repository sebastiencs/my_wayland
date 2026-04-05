precision highp float;
varying vec2 v_texcoord;
uniform sampler2D u_texture;
uniform vec2 u_direction;
uniform vec2 u_texel_size;
uniform float u_radius;
void main() {
    float radius = max(u_radius, 1.0);
    vec2 offset1 = u_direction * u_texel_size * (1.384615 * radius);
    vec2 offset2 = u_direction * u_texel_size * (3.230769 * radius);
    vec4 color = texture2D(u_texture, v_texcoord) * 0.227027;
    color += texture2D(u_texture, v_texcoord + offset1) * 0.316216;
    color += texture2D(u_texture, v_texcoord - offset1) * 0.316216;
    color += texture2D(u_texture, v_texcoord + offset2) * 0.070270;
    color += texture2D(u_texture, v_texcoord - offset2) * 0.070270;
    gl_FragColor = color;
}
